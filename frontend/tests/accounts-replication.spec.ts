import { expect, test } from '@playwright/test';

const user = { id: 'user-1', username: 'admin', display_name: 'Administrator', role: 'superadmin', disabled: false };
const session = { authenticated: true, user, csrf_token: 'csrf-test-token' };
const accounts = [
  { id: 'primary', label: 'Primary', phone: '+1000', state: 'connected', storage_chat_id: '-1001', replica_objects: 0, access_objects: 0, created_at: 1, updated_at: 1 },
  { id: 'backup', label: 'Backup', phone: '+1000', state: 'configured', storage_chat_id: '-1002', replica_objects: 0, access_objects: 0, created_at: 2, updated_at: 2 }
];

async function mockConsole(page: import('@playwright/test').Page) {
  let savedAccount = false;
  let replicationBody: Record<string, unknown> | null = null;
  let rechunkBody: Record<string, unknown> | null = null;
  await page.route('**/_admin/api/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace('/_admin/api', '');
    if (path === '/session' && request.method() === 'GET') return route.fulfill({ json: session });
    if (path === '/setup') return route.fulfill({ json: { setup_required: false } });
    if (path === '/overview') return route.fulfill({ json: { telegram: { connection_state: 'connected', detail: 'mock', session_state: 'authorized' }, recovery: { issue_count: 0, unacknowledged_count: 0, issues: [] }, storage: {}, checks: [] } });
    if (path === '/telegram/settings') return route.fulfill({ json: { settings: { telegram_api_id: '123', telegram_api_hash: 'hash', telegram_storage_chat_id: '-1001', telegram_proxy_url: '', telegram_proxy_username: '', telegram_proxy_password: '', telegram_proxy_mode: 'auto' } } });
    if (path === '/accounts' && request.method() === 'GET') return route.fulfill({ json: { accounts } });
    if (path === '/accounts' && request.method() === 'POST') { savedAccount = true; return route.fulfill({ json: { account: { ...accounts[1], label: 'New backup' } } }); }
    if (path === '/replication' && request.method() === 'GET') return route.fulfill({ json: { jobs: [] } });
    if (path === '/replication' && request.method() === 'POST') { replicationBody = request.postDataJSON(); return route.fulfill({ status: 202, json: { job: { id: 'replication-1', bucket: 'release-test', state: 'queued', mode: 'automatic', access_mode: 'access', chunks_done: 0, chunks_total: 0, objects_done: 0, objects_total: 0, bytes_done: 0 } } }); }
    if (path === '/rechunk' && request.method() === 'GET') return route.fulfill({ json: { jobs: [] } });
    if (path === '/rechunk' && request.method() === 'POST') { rechunkBody = request.postDataJSON(); return route.fulfill({ status: 202, json: { jobs: [] } }); }
    if (path === '/buckets' && request.method() === 'GET') return route.fulfill({ json: { buckets: [{ name: 'release-test', created_at: '2026-01-01T00:00:00Z' }], page: 1, page_size: 25, total: 1, has_more: false } });
    if (path === '/objects' && request.method() === 'GET') return route.fulfill({ json: { prefix: '', folders: [], objects: [{ key: 'sample.bin', name: 'sample.bin', size: 10, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 }], page: 1, page_size: 25, total: 1, has_more: false } });
    if (path === '/objects/delete' && request.method() === 'POST') return route.fulfill({ json: { ok: true } });
    return route.fulfill({ json: {} });
  });
  return { get savedAccount() { return savedAccount; }, get replicationBody() { return replicationBody; }, get rechunkBody() { return rechunkBody; } };
}

test('accounts page saves connections and submits automatic access replication', async ({ page }) => {
  const state = await mockConsole(page);
  await page.goto('/_admin/accounts');
  await expect(page.getByRole('heading', { name: 'Telegram account pool' })).toBeVisible();
  await page.getByLabel('Label').fill('New backup');
  await page.getByLabel('API ID').fill('123');
  await page.getByLabel('API hash').fill('hash');
  await page.getByLabel('Storage chat ID').fill('-1002');
  await page.getByRole('button', { name: 'Save connection' }).click();
  expect(state.savedAccount).toBe(true);
  await page.getByLabel('Bucket').selectOption('release-test');
  await page.getByLabel('Schedule').selectOption('automatic');
  await page.getByLabel('Target mode').selectOption('access');
  await page.getByRole('button', { name: 'Queue replication' }).click();
  await expect(page.getByText('Replication job queued')).toBeVisible();
  expect(state.replicationBody).toMatchObject({ bucket: 'release-test', mode: 'automatic', access_mode: 'access' });
});

test('bucket selection opens the re-chunk size dialog and queues selected keys', async ({ page }) => {
  const state = await mockConsole(page);
  await page.goto('/_admin/buckets');
  await page.getByRole('button', { name: /^release-test/ }).click();
  await page.getByRole('checkbox', { name: 'Select sample.bin' }).check();
  await page.getByRole('button', { name: 'Re-chunk' }).click();
  await expect(page.getByRole('heading', { name: 'Re-chunk selected files' })).toBeVisible();
  await page.getByLabel('New chunk size (MiB)').fill('4');
  await Promise.all([
    page.waitForRequest((request) => request.method() === 'POST' && new URL(request.url()).pathname.endsWith('/rechunk')),
    page.getByRole('button', { name: 'Queue re-chunking' }).click(),
  ]);
  expect(state.rechunkBody).toMatchObject({ bucket: 'release-test', keys: ['sample.bin'], new_chunk_size: 4 * 1048576 });
});
