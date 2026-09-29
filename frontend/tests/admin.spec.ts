import { expect, test } from '@playwright/test';

const user = {
  id: 'user-1',
  username: 'admin',
  display_name: 'Administrator',
  role: 'superadmin',
  disabled: false
};

const authenticated = {
  authenticated: true,
  user,
  csrf_token: 'csrf-test-token',
  issued_at: '2026-01-01T00:00:00Z',
  expires_at: '2099-01-01T00:00:00Z'
};

const overview = {
  checked_at: '2026-01-01T00:00:00Z',
  session: { authenticated: true, user },
  storage: {
    buckets: 1,
    committed_objects: 1,
    active_objects: 1,
    staged_objects: 0,
    recovery_markers: 0,
    chunk_size: 1_048_576,
    recovery_required_objects: 0,
    telegram_files_bytes: 256 * 1024 * 1024
  },
  recovery: {
    issue_count: 0,
    unacknowledged_count: 0,
    scan_ok: true,
    issues: []
  },
  transfers: {
    pending_jobs: 2,
    oldest_pending_age_seconds: 3720,
    retries: 5,
    failed_jobs: 1,
    staging_bytes: 4_194_304,
    cleanup_backlog: 3,
    cleanup_recovery_required: 1
  },
  traffic: {
    session: {
      client_upload_bytes: 1_048_576,
      client_download_bytes: 8_388_608,
      telegram_upload_bytes: 4_194_304,
      telegram_download_bytes: 16_777_216
    },
    total: {
      client_upload_bytes: 11_534_336,
      client_download_bytes: 92_274_688,
      telegram_upload_bytes: 46_137_344,
      telegram_download_bytes: 167_772_160
    }
  },
  stage_metrics: {
    active_requests: 1,
    completed_requests: 12,
    failed_requests: 1,
    test_active_requests: 0,
    last_test: null,
    recent: [{
      request_id: 42,
      surface: 'public',
      started_at: '2026-01-01T00:00:00Z',
      status: 'completed',
      chunks: 3,
      client_bytes: 8_388_608,
      telegram_bytes: 8_400_000,
      telegram_retries: 1,
      first_chunk_us: 1_250_000,
      telegram_us: 3_200_000,
      retry_wait_us: 50_000,
      decrypt_us: 1_200,
      verify_us: 2_400,
      total_us: 5_100_000,
      error: null
    }]
  },
  telegram: {
    session_state: 'authorized',
    connection_state: 'connected',
    detail: 'mock storage chat reachable',
    storage_chat_id: '-1001234567890'
  },
  checks: [
    { label: 'Telegram storage', ok: true, detail: 'mock storage chat reachable' },
    { label: 'Storage chat', ok: true, detail: 'resolved from Telegram bootstrap settings' },
    { label: 'UI assets', ok: true, detail: 'Svelte build output present' }
  ]
};

async function mockAdminApi(
  page: import('@playwright/test').Page,
  options: {
    telegramSettingsFailure?: boolean;
    storageSettingsFailure?: boolean;
    recoveryIssues?: Array<Record<string, unknown>>;
    delayFirstObjectListMs?: number;
    delayNestedObjectListMs?: number;
    staleCsrfOnce?: boolean;
    expireOnNextBuckets?: boolean;
    browserBuckets?: Array<{ name: string; created_at: string }>;
    browserObjects?: Array<Record<string, unknown>>;
    telegramAccounts?: Array<{ id: string; label: string; state: string; detail: string; connected: boolean; download_enabled: boolean }>;
  } = {}
) {
  let loggedIn = false;
  let csrfToken = authenticated.csrf_token;
  let staleCsrfRejected = false;
  let sessionExpired = false;
  let recoveryJobVisible = true;
  let connectionRemoved = false;
  let chunkSize = 1_048_576;
  let downloadPrefetchChunks = 1;
  const deletedKeys = new Set<string>();
  const buckets = options.browserBuckets ?? [{ name: 'release-test', created_at: '2026-01-01T00:00:00Z' }];
  const users = [user];
  let delayedFirstObjectList = false;
  let delayedNestedObjectList = false;
  let stageTestSample: Record<string, unknown> | null = null;
  const rechunkRequests: Array<Record<string, unknown>> = [];
  const accountRows = [{ id: 'primary', label: 'Primary account', phone: '+15551234567', state: 'configured', storage_chat_id: '-1001234567890', replica_objects: 0, access_objects: 0, download_enabled: true, created_at: 1, updated_at: 1 }];
  await page.route('**/_admin/api/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace('/_admin/api', '');
    if (path === '/session' && request.method() === 'GET') {
      return route.fulfill({ json: loggedIn && !sessionExpired ? { ...authenticated, csrf_token: csrfToken } : { authenticated: false } });
    }
    if (path === '/session/login' && request.method() === 'POST') {
      loggedIn = true;
      csrfToken = authenticated.csrf_token;
      return route.fulfill({ json: { ...authenticated, csrf_token: csrfToken } });
    }
    if (path === '/session/logout' && request.method() === 'POST') {
      loggedIn = false;
      return route.fulfill({ json: { authenticated: false } });
    }
    if (path === '/setup' && request.method() === 'GET') {
      return route.fulfill({ json: { setup_required: false } });
    }
    if (options.expireOnNextBuckets && path === '/buckets' && request.method() === 'GET' && !sessionExpired) {
      sessionExpired = true;
      return route.fulfill({ status: 401, json: { error: 'session expired' } });
    }
    if (!loggedIn || sessionExpired) return route.fulfill({ status: 401, json: { error: 'unauthorized' } });
    if (path === '/stage-metrics/test' && request.method() === 'POST') {
      stageTestSample = {
        request_id: 99, surface: 'diagnostic-test', started_at: '2026-01-01T00:00:00Z', status: 'completed',
        chunks: 1, client_bytes: 1_048_576, telegram_bytes: 1_050_000, telegram_retries: 0,
        first_chunk_us: 2_000_000, telegram_us: 1_800_000, retry_wait_us: 0, decrypt_us: 1_100,
        verify_us: 2_100, total_us: 3_900_000, error: null
      };
      return route.fulfill({ json: { ok: true, sample: stageTestSample } });
    }
    if (path === '/overview/live') {
      const overviewTelegram = options.telegramAccounts ? { ...overview.telegram, connection_state: options.telegramAccounts.every((account) => account.connected) ? 'connected' : options.telegramAccounts.some((account) => account.connected) ? 'partial' : 'disconnected', accounts: options.telegramAccounts } : overview.telegram;
      return route.fulfill({ json: connectionRemoved ? { checked_at: overview.checked_at, telegram: { ...overviewTelegram, connection_state: 'needs_reauth', detail: 'Telegram storage is not connected' }, transfers: overview.transfers, traffic: overview.traffic, stage_metrics: { ...overview.stage_metrics, last_test: stageTestSample }, checks: overview.checks } : { checked_at: overview.checked_at, telegram: overviewTelegram, transfers: overview.transfers, traffic: overview.traffic, stage_metrics: { ...overview.stage_metrics, last_test: stageTestSample }, checks: overview.checks } });
    }
    if (path === '/overview') {
      const recoveryIssues = options.recoveryIssues ?? [];
      const clearedOverview = {
        ...overview,
        storage: { ...overview.storage, chunk_size: chunkSize, recovery_required_objects: 0 },
        recovery: { ...overview.recovery, issue_count: recoveryIssues.length, unacknowledged_count: recoveryIssues.length, issues: recoveryIssues },
        telegram: { ...overview.telegram, connection_state: 'needs_reauth', detail: 'Telegram storage is not connected' }
      };
      const overviewTelegram = options.telegramAccounts ? { ...overview.telegram, connection_state: options.telegramAccounts.every((account) => account.connected) ? 'connected' : options.telegramAccounts.some((account) => account.connected) ? 'partial' : 'disconnected', accounts: options.telegramAccounts } : overview.telegram;
      return route.fulfill({ json: connectionRemoved ? clearedOverview : { ...overview, stage_metrics: { ...overview.stage_metrics, last_test: stageTestSample }, storage: { ...overview.storage, chunk_size: chunkSize }, recovery: { ...overview.recovery, issue_count: recoveryIssues.length, unacknowledged_count: recoveryIssues.length, issues: recoveryIssues }, telegram: overviewTelegram } });
    }
    if (path === '/telegram/disconnect' && request.method() === 'POST') {
      connectionRemoved = true;
      recoveryJobVisible = false;
      return route.fulfill({ status: 202, json: { ok: true, job: { id: 'removal-1', state: 'pending', delete_uploaded_files: false }, message: 'Connection removed.' } });
    }
    if (path === '/telegram/settings' && request.method() === 'POST') {
      if (options.telegramSettingsFailure) {
        return route.fulfill({ status: 400, json: { error: 'telegram settings rejected for this test' } });
      }
      const body = request.postDataJSON() as Record<string, string>;
      return route.fulfill({
        json: {
          settings: {
            telegram_api_id: body.telegram_api_id ?? '12345',
            telegram_api_hash: body.telegram_api_hash ?? 'hash',
            telegram_storage_chat_id: body.telegram_storage_chat_id ?? '-1001234567890',
            telegram_proxy_url: body.telegram_proxy_url ?? '',
            telegram_proxy_username: body.telegram_proxy_username ?? '',
            telegram_proxy_password: body.telegram_proxy_password ?? '',
            telegram_proxy_mode: body.telegram_proxy_mode ?? 'auto',
            telegram_account_phone: '+15551234567'
          }
        }
      });
    }
    if (path === '/telegram/settings' && request.method() === 'GET') {
      return route.fulfill({
        json: {
          settings: {
            telegram_api_id: '12345',
            telegram_api_hash: 'hash',
            telegram_storage_chat_id: '-1001234567890',
            telegram_proxy_url: '',
            telegram_proxy_username: '',
            telegram_proxy_password: '',
            telegram_proxy_mode: 'auto',
            telegram_account_phone: '+15551234567'
          }
        }
      });
    }
    if (path === '/telegram/storage-settings' && request.method() === 'GET') {
      return route.fulfill({ json: { chunk_size: chunkSize, min_chunk_size: 1, max_chunk_size: 2_000_000_000, download_prefetch_chunks: downloadPrefetchChunks, min_download_prefetch_chunks: 0, max_download_prefetch_chunks: 4, recovery_verify_enabled: true, recovery_verify_startup: true, recovery_verify_interval_secs: 300, min_recovery_verify_interval_secs: 60, max_recovery_verify_interval_secs: 604800, recovery_verify_chunks: 1, min_recovery_verify_chunks: 1, max_recovery_verify_chunks: 1024, cleanup_retention_secs: 43200, min_cleanup_retention_secs: 3600, max_cleanup_retention_secs: 2592000, source: 'database' } });
    }
    if (path === '/telegram/storage-settings' && request.method() === 'POST') {
      if (options.storageSettingsFailure) return route.fulfill({ status: 400, json: { error: 'storage settings rejected for this test' } });
      const body = request.postDataJSON() as { chunk_size: number; download_prefetch_chunks: number };
      chunkSize = body.chunk_size;
      downloadPrefetchChunks = body.download_prefetch_chunks;
      return route.fulfill({ json: { chunk_size: chunkSize, min_chunk_size: 1, max_chunk_size: 2_000_000_000, download_prefetch_chunks: downloadPrefetchChunks, min_download_prefetch_chunks: 0, max_download_prefetch_chunks: 4, recovery_verify_enabled: true, recovery_verify_startup: true, recovery_verify_interval_secs: 300, min_recovery_verify_interval_secs: 60, max_recovery_verify_interval_secs: 604800, recovery_verify_chunks: 1, min_recovery_verify_chunks: 1, max_recovery_verify_chunks: 1024, cleanup_retention_secs: 43200, min_cleanup_retention_secs: 3600, max_cleanup_retention_secs: 2592000, source: 'database' } });
    }
    if (path === '/telegram/wizard/begin' && request.method() === 'POST') {
      return route.fulfill({ json: { phase: 'code', message: null } });
    }
    if (path === '/telegram/wizard/submit-code' && request.method() === 'POST') {
      return route.fulfill({ json: { phase: 'two_fa', message: null } });
    }
    if (path === '/telegram/wizard/submit-password' && request.method() === 'POST') {
      return route.fulfill({ json: { phase: 'authorized', connection_ready: true, message: null } });
    }
    if (path === '/telegram/wizard/cancel' && request.method() === 'POST') {
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/accounts' && request.method() === 'GET') return route.fulfill({ json: { accounts: accountRows } });
    if (path === '/accounts' && request.method() === 'POST') {
      const body = request.postDataJSON() as Record<string, unknown>;
      const account = { id: String(body.id ?? `account-${accountRows.length + 1}`), label: String(body.label ?? 'Account'), phone: body.phone ?? null, state: 'configured', storage_chat_id: body.telegram_storage_chat_id ?? '-1001234567890', replica_objects: 0, access_objects: 0, download_enabled: body.download_enabled !== false, created_at: 2, updated_at: 2 };
      const existing = accountRows.findIndex((item) => item.id === account.id);
      if (existing >= 0) accountRows[existing] = account;
      else accountRows.push(account);
      return route.fulfill({ json: { account, refresh_error: null } });
    }
    if (path === '/replication' && request.method() === 'GET') return route.fulfill({ json: { jobs: [] } });
    if (path === '/rechunk' && request.method() === 'GET') return route.fulfill({ json: { jobs: [] } });
    if (path === '/rechunk' && request.method() === 'POST') { rechunkRequests.push(request.postDataJSON() as Record<string, unknown>); return route.fulfill({ status: 202, json: { jobs: [] } }); }
    if (path === '/replicas' && request.method() === 'GET') return route.fulfill({ json: { replicas: [] } });
    if (path === '/buckets' && request.method() === 'GET') {
      const params = new URL(request.url()).searchParams;
      const search = (params.get('search') ?? '').toLowerCase();
      const page = Number(params.get('page') ?? '1');
      const pageSize = Number(params.get('page_size') ?? '25');
      const sort = params.get('sort') ?? 'name';
      const order = params.get('order') === 'desc' ? -1 : 1;
      const matching = buckets.filter((bucket) => bucket.name.toLowerCase().includes(search)).sort((a, b) => {
        const left = sort === 'created_at' ? a.created_at : a.name;
        const right = sort === 'created_at' ? b.created_at : b.name;
        return left.localeCompare(right) * order;
      });
      const start = (page - 1) * pageSize;
      return route.fulfill({ json: { buckets: matching.slice(start, start + pageSize), page, page_size: pageSize, total: matching.length, has_more: start + pageSize < matching.length, search } });
    }
    if (path === '/search' && request.method() === 'GET') {
      const params = new URL(request.url()).searchParams;
      const search = (params.get('search') ?? '').toLowerCase();
      const page = Number(params.get('page') ?? '1');
      const pageSize = Number(params.get('page_size') ?? '25');
      const allObjects = options.browserObjects ?? [{ key: 'readme.txt', name: 'readme.txt', size: 12, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 }];
      const matching = buckets.flatMap((bucket) => allObjects
        .filter((object) => `${bucket.name}/${String(object.key)}`.toLowerCase().includes(search))
        .map((object) => ({ ...object, bucket: bucket.name, location: String(object.key).includes('/') ? `${String(object.key).slice(0, String(object.key).lastIndexOf('/') + 1)}` : null })));
      const start = (page - 1) * pageSize;
      return route.fulfill({ json: { results: matching.slice(start, start + pageSize), page, page_size: pageSize, total: matching.length, has_more: start + pageSize < matching.length, search } });
    }
    if (path === '/buckets' && request.method() === 'POST') {
      if (options.staleCsrfOnce && !staleCsrfRejected) {
        staleCsrfRejected = true;
        csrfToken = 'csrf-refreshed-token';
        return route.fulfill({ status: 403, json: { error: 'invalid csrf token' } });
      }
      const body = request.postDataJSON() as { name: string };
      if (body.name === '_public' || body.name === '_admin') {
        return route.fulfill({ status: 400, json: { error: `${body.name} is reserved for an internal HTTP route` } });
      }
      buckets.push({ name: body.name, created_at: '2026-01-01T00:00:00Z' });
      return route.fulfill({ json: buckets.at(-1) });
    }
    if (path.startsWith('/buckets/') && request.method() === 'DELETE') {
      const name = decodeURIComponent(path.slice('/buckets/'.length));
      const index = buckets.findIndex((bucket) => bucket.name === name);
      if (index >= 0) buckets.splice(index, 1);
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/objects' && request.method() === 'GET') {
      const params = new URL(request.url()).searchParams;
      const prefix = params.get('prefix') ?? '';
      const search = (params.get('search') ?? '').toLowerCase();
      if (options.delayFirstObjectListMs && !delayedFirstObjectList) {
        delayedFirstObjectList = true;
        await new Promise((resolve) => setTimeout(resolve, options.delayFirstObjectListMs));
      }
      if (prefix && options.delayNestedObjectListMs && !delayedNestedObjectList) {
        delayedNestedObjectList = true;
        await new Promise((resolve) => setTimeout(resolve, options.delayNestedObjectListMs));
      }
      const rootObjects = options.browserObjects ?? [{ key: 'readme.txt', name: 'readme.txt', size: 12, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 }];
      const nestedObjects = [{ key: 'docs/report.txt', name: 'report.txt', size: 24, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 }];
      const allObjects = (prefix ? nestedObjects : rootObjects).filter((object) => !deletedKeys.has(String(object.key)));
      const matching = search ? allObjects.filter((object) => String(object.key).toLowerCase().includes(search)).map((object) => ({ ...object, location: String(object.key).includes('/') ? `${String(object.key).slice(0, String(object.key).lastIndexOf('/') + 1)}` : null })) : allObjects;
      const page = Number(params.get('page') ?? '1');
      const pageSize = Number(params.get('page_size') ?? '25');
      const sort = params.get('sort') ?? 'name';
      const order = params.get('order') === 'desc' ? -1 : 1;
      matching.sort((a, b) => {
        const left = sort === 'size' ? Number(a.size ?? 0) : sort === 'last_modified' ? String(a.last_modified ?? '') : String(a.name ?? a.key);
        const right = sort === 'size' ? Number(b.size ?? 0) : sort === 'last_modified' ? String(b.last_modified ?? '') : String(b.name ?? b.key);
        return (typeof left === 'number' && typeof right === 'number' ? left - right : String(left).localeCompare(String(right))) * order;
      });
      const start = (page - 1) * pageSize;
      return route.fulfill({ json: { prefix, folders: search ? [] : (prefix ? [] : ['docs']), objects: matching.slice(start, start + pageSize), page, page_size: pageSize, total: (search ? matching : (prefix ? nestedObjects : [...new Set(['docs', ...allObjects.map((object) => String(object.key).includes('/') ? String(object.key).split('/')[0] : '')].filter(Boolean))])).length, has_more: start + pageSize < matching.length, search } });
    }
    if (path === '/objects/share' && request.method() === 'POST') {
      return route.fulfill({ status: 201, json: { url: '/_public/mock-share-token', expires_at: '2026-01-01T01:00:00Z' } });
    }
    if (path === '/objects/delete' && request.method() === 'POST') {
      const body = request.postDataJSON() as { key: string };
      deletedKeys.add(body.key);
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/objects/folder' && request.method() === 'POST') return route.fulfill({ json: { ok: true } });
    if (path === '/users' && request.method() === 'GET') return route.fulfill({ json: { users } });
    if (path.startsWith('/users/') && request.method() === 'DELETE') return route.fulfill({ json: { ok: true } });
    if (path === '/recovery/acknowledge' || path === '/recovery/unacknowledge') return route.fulfill({ json: { ok: true, acknowledged_count: 1, unacknowledged_count: 0 } });
    if (path.startsWith('/jobs/') && path.endsWith('/retry')) {
      recoveryJobVisible = false;
      return route.fulfill({ json: { ok: true } });
    }
    if (path.startsWith('/jobs')) {
      return route.fulfill({
        json: {
          jobs: recoveryJobVisible
            ? [
                {
                  id: 'job-1',
                  object_id: 'object-1',
                  operation_id: 'operation-1',
                  bucket: 'release-test',
                  key: 'stuck/installer.bin',
                  state: 'recovery_required',
                  bytes: 295_682_132,
                  chunks_done: 236,
                  chunks_total: 282,
                  attempts: 2,
                  next_retry: 0,
                  error: 'Telegram acknowledgement is unknown; automatic reconciliation is pending',
                  created_at: 1_767_000_000,
                  updated_at: 1_767_000_100
                }
              ]
            : [],
          next_offset: null
        }
      });
    }
    return route.fulfill({ json: {} });
  });
  return { rechunkRequests };
}

test('recovers from a stale CSRF token without requiring a page refresh', async ({ page }) => {
  const bucketCsrfHeaders: string[] = [];
  await mockAdminApi(page, { staleCsrfOnce: true });
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname === '/_admin/api/buckets' && request.method() === 'POST') {
      bucketCsrfHeaders.push(request.headers()['x-csrf-token'] ?? '');
    }
  });

  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await expect(page.getByRole('heading', { name: 'Your buckets' })).toBeVisible();
  await page.getByRole('button', { name: /Create bucket/ }).first().click();
  await page.getByLabel('Bucket name').fill('csrf-recovered');
  await page.getByRole('button', { name: 'Create bucket' }).last().click();

  await expect(page.getByRole('button', { name: /csrf-recovered created/ })).toBeVisible();
  expect(bucketCsrfHeaders).toEqual(['csrf-test-token', 'csrf-refreshed-token']);
});

test('reserved internal route bucket names are rejected by the bucket form', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();

  for (const name of ['_public', '_admin']) {
    await page.getByRole('button', { name: /Create bucket/ }).first().click();
    await page.getByLabel('Bucket name').fill(name);
    await page.locator('.compact-modal').getByRole('button', { name: 'Create bucket', exact: true }).click();
    await expect(page.getByRole('alert').filter({ hasText: `${name} is reserved` })).toBeVisible();
    await expect(page.getByRole('button', { name: new RegExp(`${name} created`) })).toHaveCount(0);
  }
});

test('guest is gated, authenticated navigation works, and logout revokes the session', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('/');

  await expect(page.getByRole('heading', { name: 'Sign in to manage storage' })).toBeVisible();
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page.getByRole('heading', { name: 'Storage at a glance' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Transfer pipeline' })).toBeVisible();
  await expect(page.getByLabel('Transfer pipeline chart')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Traffic since process start' })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'This session' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByText('Telegram files')).toBeVisible();
  await expect(page.getByText('256 MiB')).toBeVisible();
  await expect(page.getByText('Clients → server')).toBeVisible();
  await expect(page.getByText('Telegram → server')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Download stage metrics' })).toBeVisible();
  await expect(page.getByRole('table', { name: 'Recent download stage timings' })).toBeVisible();
  const stageTestRequest = page.waitForRequest((request) => new URL(request.url()).pathname === '/_admin/api/stage-metrics/test' && request.method() === 'POST');
  await page.getByRole('button', { name: 'Run test' }).click();
  await stageTestRequest;
  await expect(page.getByLabel('Last diagnostic stage test')).toContainText('diagnostic test');
  await expect(page.getByText('public', { exact: true })).toBeVisible();
  await expect(page.getByText('1.3s', { exact: true })).toBeVisible();
  await page.getByRole('tab', { name: 'Total' }).click();
  await expect(page.getByRole('heading', { name: 'Traffic across all server runs' })).toBeVisible();
  await expect(page.getByRole('tab', { name: 'Total' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByText('11 MiB')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'System checks' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Storage safeguards' })).toBeVisible();
  await expect(page.locator('.account-avatar')).toHaveText('A');
  await expect(page.locator('.account-kicker')).toHaveText('Signed in as');
  await expect(page.getByRole('button', { name: 'Sign out' }).locator('svg')).toBeVisible();
  await page.getByRole('button', { name: 'Storage settings' }).click();
  await expect(page.getByRole('heading', { name: 'Give every upload the right-sized runway.' })).toBeVisible();
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to manage storage' })).toBeVisible();
});

test('overview polls lightweight live telemetry without rescanning the full snapshot', async ({ page }) => {
  let fullOverviewRequests = 0;
  let liveOverviewRequests = 0;
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (path.endsWith('/_admin/api/overview')) fullOverviewRequests += 1;
    if (path.endsWith('/_admin/api/overview/live')) liveOverviewRequests += 1;
  });
  await mockAdminApi(page);
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'Traffic since process start' })).toBeVisible();

  const fullRequestsAfterInitialLoad = fullOverviewRequests;
  await expect.poll(() => liveOverviewRequests, { timeout: 6_500 }).toBeGreaterThan(0);
  await expect.poll(() => fullOverviewRequests, { timeout: 1_000 }).toBe(fullRequestsAfterInitialLoad);
});

test('expired authentication returns the operator to login instead of an API error page', async ({ page }) => {
  await mockAdminApi(page, { expireOnNextBuckets: true });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();

  await expect(page.getByRole('heading', { name: 'Sign in to manage storage' })).toBeVisible();
  await expect(page.getByText('Not authenticated')).toHaveCount(0);
});

test('responsive navigation remains usable on a narrow viewport', async ({ page }) => {
  await mockAdminApi(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Transfers' })).toBeVisible();
});

test('the console stays within the viewport across phone, tablet, and desktop widths', async ({ page }) => {
  await mockAdminApi(page);
  await page.setViewportSize({ width: 320, height: 800 });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();

  async function expectNoHorizontalOverflow() {
    const dimensions = await page.evaluate(() => ({
      viewport: document.documentElement.clientWidth,
      documentWidth: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth)
    }));
    expect(dimensions.documentWidth).toBeLessThanOrEqual(dimensions.viewport + 1);
  }

  for (const width of [320, 390, 768, 1024, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    await page.getByRole('button', { name: 'Overview' }).click();
    await expect(page.getByRole('heading', { name: 'Storage at a glance' })).toBeVisible();
    await expectNoHorizontalOverflow();

    await page.getByRole('button', { name: 'Buckets' }).click();
    await expect(page.getByRole('heading', { name: 'Your buckets' })).toBeVisible();
    await expectNoHorizontalOverflow();

    await page.getByRole('button', { name: 'Transfers' }).click();
    await expect(page.getByRole('heading', { name: 'Transfer activity' })).toBeVisible();
    await expectNoHorizontalOverflow();

    await page.getByRole('button', { name: 'Storage settings' }).click();
    await expect(page.getByRole('heading', { name: 'Give every upload the right-sized runway.' })).toBeVisible();
    await expectNoHorizontalOverflow();
  }

  await page.setViewportSize({ width: 320, height: 900 });
  await page.getByRole('button', { name: 'Accounts' }).click();
  await page.getByRole('button', { name: 'Edit account setup' }).click();
  await expect(page.getByRole('heading', { name: 'Start with your Telegram app' })).toBeVisible();
  await expectNoHorizontalOverflow();
});

test('re-entering a bucket reloads its objects', async ({ page }) => {
  await mockAdminApi(page);
  let objectListRequests = 0;
  page.on('request', (request) => {
    if (new URL(request.url()).pathname === '/_admin/api/objects') objectListRequests += 1;
  });

  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await page.getByRole('button', { name: /^release-test created/ }).click();

  await expect(page.getByText('readme.txt')).toBeVisible();
  expect(objectListRequests).toBe(1);

  await page.getByRole('button', { name: 'All buckets' }).click();
  await page.getByRole('button', { name: /^release-test created/ }).click();

  await expect(page.getByText('readme.txt')).toBeVisible();
  expect(objectListRequests).toBe(2);
});

test('bucket and object search supports pagination and direct folder navigation', async ({ page }) => {
  const browserBuckets = Array.from({ length: 27 }, (_, index) => ({
    name: `archive-${String(index).padStart(2, '0')}`,
    created_at: '2026-01-01T00:00:00Z'
  }));
  const browserObjects = [
    { key: 'archive/reports/final-report.txt', name: 'final-report.txt', size: 42, last_modified: '2026-01-01T00:00:00Z', shared_links: 2 },
    { key: 'archive/readme.txt', name: 'readme.txt', size: 8, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 }
  ];
  await mockAdminApi(page, { browserBuckets, browserObjects });
  const bucketRequests: string[] = [];
  const objectRequests: string[] = [];
  const searchRequests: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname === '/_admin/api/buckets' && request.method() === 'GET') bucketRequests.push(url.search);
    if (url.pathname === '/_admin/api/search' && request.method() === 'GET') searchRequests.push(url.search);
    if (url.pathname === '/_admin/api/objects' && request.method() === 'GET') objectRequests.push(url.search);
  });

  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await expect(page.getByRole('button', { name: /archive-00 created/ })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Go to bucket page 1' })).toHaveAttribute('aria-current', 'page');
  await page.getByRole('button', { name: 'Go to bucket page 2' }).click();
  await expect(page.getByRole('button', { name: /archive-26 created/ })).toBeVisible();
  expect(bucketRequests.at(-1)).toContain('page=2');
  await page.getByRole('searchbox', { name: 'Search buckets and files' }).fill('archive-26');
  await expect(page.getByRole('button', { name: /archive-26 created/ })).toBeVisible();
  await expect.poll(() => bucketRequests.at(-1) ?? '').toContain('search=archive-26');
  await expect.poll(() => searchRequests.at(-1) ?? '').toContain('search=archive-26');
  await expect(page.getByRole('region', { name: 'Recursive file search' })).toContainText('final-report.txt');

  await page.getByRole('button', { name: /archive-26 created/ }).click();
  await expect(page.getByRole('searchbox', { name: 'Search objects and folders' })).toBeVisible();
  await page.getByRole('searchbox', { name: 'Search objects and folders' }).fill('final-report');
  await expect(page.getByText('final-report.txt')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Go to folder archive/reports/' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Download final-report.txt' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Share final-report.txt' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Manage shared links for final-report.txt' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Delete final-report.txt' })).toBeVisible();
  await expect.poll(() => objectRequests.at(-1) ?? '').toContain('search=final-report');
  expect(objectRequests.at(-1)).not.toContain('delimiter=1');
  await page.getByRole('button', { name: 'Go to folder archive/reports/' }).click();
  await expect(page.getByRole('heading', { name: 'Bucket / archive-26 / archive / reports' })).toBeVisible();
});

test('bucket and folder columns sort across the paginated API listing', async ({ page }) => {
  const browserBuckets = [
    { name: 'zulu', created_at: '2026-02-01T00:00:00Z' },
    { name: 'alpha', created_at: '2026-01-01T00:00:00Z' }
  ];
  const browserObjects = [
    { key: 'large.bin', name: 'large.bin', size: 40, last_modified: '2026-01-02T00:00:00Z', shared_links: 0 },
    { key: 'small.bin', name: 'small.bin', size: 10, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 }
  ];
  await mockAdminApi(page, { browserBuckets, browserObjects });
  const bucketRequests: string[] = [];
  const objectRequests: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname === '/_admin/api/buckets' && request.method() === 'GET') bucketRequests.push(url.search);
    if (url.pathname === '/_admin/api/objects' && request.method() === 'GET') objectRequests.push(url.search);
  });

  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();

  await expect(page.locator('.bucket-table tbody tr').first()).toContainText('alpha');
  await page.getByRole('button', { name: 'Sort buckets by Name Sort descending' }).click();
  await expect.poll(() => bucketRequests.at(-1) ?? '').toContain('sort=name&order=desc');
  await expect(page.locator('.bucket-table tbody tr').first()).toContainText('zulu');

  await page.getByRole('button', { name: /^zulu created/ }).click();
  await expect(page.getByRole('table').last()).toBeVisible();
  await page.getByRole('button', { name: 'Sort objects by Size Sort ascending' }).click();
  await expect.poll(() => objectRequests.at(-1) ?? '').toContain('sort=size&order=asc');
  await expect.poll(() => page.locator('.object-name').allTextContents()).toEqual(['small.bin', 'large.bin']);

  await page.getByRole('button', { name: 'Sort objects by Size Sort descending' }).click();
  await expect.poll(() => objectRequests.at(-1) ?? '').toContain('sort=size&order=desc');
  await expect.poll(() => page.locator('.object-name').allTextContents()).toEqual(['large.bin', 'small.bin']);
  const row = page.locator('.kv-table tbody tr').filter({hasText: 'large.bin'}).first();
  const modifiedBox = await row.locator('td').nth(3).boundingBox();
  const actionsBox = await row.locator('td').nth(4).boundingBox();
  expect(modifiedBox).not.toBeNull();
  expect(actionsBox).not.toBeNull();
  expect(modifiedBox!.x + modifiedBox!.width).toBeLessThanOrEqual(actionsBox!.x + 1);
});

test('bucket list supports bulk selection with a guarded bulk delete', async ({ page }) => {
  await mockAdminApi(page, { browserBuckets: [
    { name: 'photos', created_at: '2026-01-01T00:00:00Z' },
    { name: 'archives', created_at: '2026-01-02T00:00:00Z' }
  ] });
  const deleteRequests: string[] = [];
  page.on('request', (request) => {
    if (request.method() === 'DELETE' && new URL(request.url()).pathname.startsWith('/_admin/api/buckets/')) deleteRequests.push(new URL(request.url()).pathname);
  });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await page.getByRole('checkbox', { name: 'Select bucket photos' }).check();
  await page.getByRole('checkbox', { name: 'Select bucket archives' }).check();
  await expect(page.getByText('2 selected')).toBeVisible();
  await page.getByRole('button', { name: 'Delete buckets' }).click();
  await expect(page.getByRole('heading', { name: 'Delete selected buckets?' })).toBeVisible();
  await page.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect.poll(() => deleteRequests.length).toBe(2);
  await expect(page.getByText('No buckets yet. Create one above to start the file browser.')).toBeVisible();
});

test('bucket bulk selection can queue re-chunking for every committed object', async ({ page }) => {
  const state = await mockAdminApi(page, {
    browserBuckets: [
      { name: 'photos', created_at: '2026-01-01T00:00:00Z' },
      { name: 'archives', created_at: '2026-01-02T00:00:00Z' }
    ],
    browserObjects: [
      { key: 'one.bin', name: 'one.bin', size: 12, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 },
      { key: 'two.bin', name: 'two.bin', size: 24, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 }
    ]
  });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await page.getByRole('checkbox', { name: 'Select bucket photos' }).check();
  await page.getByRole('checkbox', { name: 'Select bucket archives' }).check();
  await page.getByRole('button', { name: 'Re-chunk files' }).click();
  await expect(page.getByRole('heading', { name: 'Re-chunk files in selected buckets' })).toBeVisible();
  await page.getByLabel('New chunk size (MiB)').fill('4');
  await page.getByRole('button', { name: 'Queue re-chunking' }).click();
  await expect.poll(() => state.rechunkRequests.length).toBe(2);
  expect(state.rechunkRequests).toEqual(expect.arrayContaining([
    expect.objectContaining({ bucket: 'photos', keys: ['one.bin', 'two.bin'], new_chunk_size: 4 * 1048576 }),
    expect.objectContaining({ bucket: 'archives', keys: ['one.bin', 'two.bin'], new_chunk_size: 4 * 1048576 })
  ]));
});

test('account cards use one add/edit form and persist download eligibility', async ({ page }) => {
  await mockAdminApi(page);
  let saved: Record<string, unknown> | null = null;
  page.on('request', (request) => {
    if (new URL(request.url()).pathname === '/_admin/api/accounts' && request.method() === 'POST') saved = request.postDataJSON() as Record<string, unknown>;
  });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Accounts' }).click();
  await page.getByRole('tab', { name: /Add account/ }).click();
  await expect(page.getByRole('heading', { name: 'Add another Telegram account' })).toBeVisible();
  await page.getByLabel('Account label').fill('Replica account');
  await page.getByLabel('Telegram API ID').fill('54321');
  await page.getByLabel('Telegram API hash').fill('replica-hash');
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByLabel('Storage chat ID').fill('-1009876543210');
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByRole('button', { name: 'Review & sign in' }).click();
  await page.getByLabel('Phone number').fill('+15550000001');
  await page.getByRole('button', { name: 'Save settings & send code' }).click();
  await page.getByLabel('Confirmation code').fill('123456');
  await page.getByRole('button', { name: 'Confirm code' }).click();
  await page.getByLabel('Cloud password').fill('password');
  await page.getByRole('button', { name: 'Authorize account' }).click();
  await expect(page.locator('.notice[role="status"]')).toContainText('authorized and added');
  expect(saved).toMatchObject({ label: 'Replica account', telegram_api_id: '54321', download_enabled: true });
  await page.getByRole('tab', { name: /Replica account/ }).click();
  await page.getByLabel('Use this account for downloads').uncheck();
  await page.getByRole('button', { name: 'Save changes' }).click();
  await expect(page.locator('.notice[role="status"]')).toContainText('Account saved');
  expect(saved).toMatchObject({ label: 'Replica account', download_enabled: false });
  await expect(page.getByRole('heading', { name: 'Edit Replica account' })).toBeVisible();
});

test('overview aggregates account health and names each account status dot', async ({ page }) => {
  await mockAdminApi(page, {
    telegramAccounts: [
      { id: 'primary', label: 'Primary storage', state: 'connected', detail: 'reachable', connected: true, download_enabled: true },
      { id: 'backup', label: 'Backup storage', state: 'disconnected', detail: 'needs reauth', connected: false, download_enabled: false }
    ]
  });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  const health = page.locator('.health');
  await expect(health).toContainText('Telegram · partial');
  await expect(health.locator('.account-dot')).toHaveCount(2);
  await expect(health.locator('.account-dot').nth(0)).toHaveAttribute('title', 'Primary storage');
  await expect(health.locator('.account-dot').nth(1)).toHaveAttribute('title', 'Backup storage');
  await expect(health.locator('.account-dot').nth(0)).toHaveClass(/connected/);
  await expect(health.locator('.account-dot').nth(1)).toHaveClass(/disconnected/);
});

test('top-level bucket search finds nested objects across buckets', async ({ page }) => {
  const browserBuckets = [
    { name: 'photos', created_at: '2026-01-01T00:00:00Z' },
    { name: 'archives', created_at: '2026-01-01T00:00:00Z' }
  ];
  const browserObjects = [
    { key: 'media/2026/report-final.pdf', name: 'report-final.pdf', size: 42, last_modified: '2026-01-01T00:00:00Z', shared_links: 0 }
  ];
  await mockAdminApi(page, { browserBuckets, browserObjects });
  const searchRequests: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname === '/_admin/api/search' && request.method() === 'GET') searchRequests.push(url.search);
  });

  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await page.getByRole('searchbox', { name: 'Search buckets and files' }).fill('report-final');
  await expect.poll(() => searchRequests.at(-1) ?? '').toContain('search=report-final');
  const results = page.getByRole('region', { name: 'Recursive file search' });
  await expect(results).toContainText('report-final.pdf');
  await expect(results).toContainText('archives / media/2026/');
  await results.getByRole('button', { name: 'Open location' }).first().click();
  await expect(page.getByRole('heading', { name: 'Bucket / photos / media / 2026' })).toBeVisible();
});

test('a slow folder load is not replaced by the background poll', async ({ page }) => {
  await mockAdminApi(page, { delayFirstObjectListMs: 10_500 });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await page.getByRole('button', { name: /^release-test created/ }).click();

  await expect(page.getByText('readme.txt')).toBeVisible({ timeout: 20_000 });
  await expect(page.getByLabel('Loading files')).toBeHidden();
});

test('folder navigation keeps the current listing visible while loading the next folder', async ({ page }) => {
  await mockAdminApi(page, { delayNestedObjectListMs: 1_200 });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await page.getByRole('button', { name: /^release-test created/ }).click();

  await expect(page.getByText('readme.txt')).toBeVisible();
  await page.getByRole('button', { name: 'docs/' }).click();

  await expect(page.getByRole('heading', { name: 'Bucket / release-test / docs' })).toBeVisible();
  await expect(page.getByText('Loading folder contents…')).toBeVisible();
  await expect(page.locator('[aria-busy="true"]')).toBeVisible();
  await expect(page.getByText('readme.txt')).toBeVisible();
  await expect(page.getByText('report.txt')).toBeVisible({ timeout: 10_000 });
  await expect(page.getByText('Loading folder contents…')).toBeHidden();
});

test('recovery transfer is visible and retry removes it after reconciliation', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Transfers' }).click();

  await expect(page.getByRole('heading', { name: 'Transfer activity' })).toBeVisible();
  await expect(page.getByText('stuck/installer.bin')).toBeVisible();
  await expect(page.getByText('Telegram acknowledgement is unknown; automatic reconciliation is pending')).toBeVisible();
  await page.getByRole('button', { name: 'Retry' }).click();
  await expect(page.getByText('No transfers yet. Upload a file from Buckets to get started.')).toBeVisible();
});

test('telegram account wizard validates, preserves, saves, and authorizes the account', async ({ page }) => {
  await mockAdminApi(page);
  const requestOrder: string[] = [];
  let savedSettings: Record<string, string> | null = null;
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (path.endsWith('/telegram/settings') && request.method() === 'POST') {
      requestOrder.push('settings');
      savedSettings = request.postDataJSON() as Record<string, string>;
    }
    if (path.endsWith('/telegram/wizard/begin') && request.method() === 'POST') requestOrder.push('begin');
  });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Accounts' }).click();
  await expect(page.getByRole('heading', { name: 'Your storage connection, beautifully in sync.' })).toBeVisible();
  await page.getByRole('button', { name: 'Edit account setup' }).click();

  await expect(page.getByRole('heading', { name: 'Start with your Telegram app' })).toBeVisible();
  await expect(page.getByText('Step 1 of 4')).toBeVisible();
  await page.getByLabel('Telegram API ID').fill('54321');
  await page.getByLabel('Telegram API hash').fill('');
  await expect(page.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await page.getByLabel('Telegram API hash').fill('updated-api-hash');
  await page.getByRole('button', { name: 'Continue' }).click();

  await expect(page.getByRole('heading', { name: 'Choose where objects live' })).toBeVisible();
  await page.locator('.wizard-page').getByLabel('Storage chat ID').fill('-1009876543210');
  await page.getByRole('button', { name: 'Continue' }).click();

  await expect(page.getByRole('heading', { name: 'Make the connection reliable' })).toBeVisible();
  await page.locator('.wizard-page').getByLabel('Connection mode').selectOption('socks5');
  await page.locator('.wizard-page').getByLabel('Proxy URL').fill('socks5://127.0.0.1:12334');
  await page.getByRole('button', { name: 'Back' }).click();
  await expect(page.locator('.wizard-page').getByLabel('Storage chat ID')).toHaveValue('-1009876543210');
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByRole('button', { name: 'Review & sign in' }).click();

  await expect(page.getByRole('heading', { name: 'Authorize the storage account' })).toBeVisible();
  await page.getByLabel('Phone number').fill('+15550000000');
  await page.getByRole('button', { name: 'Save settings & send code' }).click();
  await expect(page.getByLabel('Confirmation code')).toBeVisible();
  expect(requestOrder.indexOf('settings')).toBeGreaterThanOrEqual(0);
  expect(requestOrder.indexOf('begin')).toBeGreaterThan(requestOrder.indexOf('settings'));
  expect(savedSettings).toMatchObject({
    telegram_api_id: '54321',
    telegram_api_hash: 'updated-api-hash',
    telegram_storage_chat_id: '-1009876543210',
    telegram_proxy_url: 'socks5://127.0.0.1:12334',
    telegram_proxy_mode: 'socks5'
  });

  await page.getByLabel('Confirmation code').fill('123456');
  await page.getByRole('button', { name: 'Confirm code' }).click();
  await expect(page.getByLabel('Cloud password')).toBeVisible();

  await page.getByLabel('Cloud password').fill('test-cloud-password');
  await page.getByRole('button', { name: 'Authorize account' }).click();
  await expect(page.getByRole('heading', { name: 'Your storage connection, beautifully in sync.' })).toBeVisible();
  await expect(page.getByText('Telegram account authorized.')).toBeVisible();
});

test('telegram account wizard keeps the sign-in step open when settings cannot be saved', async ({ page }) => {
  await mockAdminApi(page, { telegramSettingsFailure: true });
  let beginRequests = 0;
  page.on('request', (request) => {
    if (new URL(request.url()).pathname.endsWith('/telegram/wizard/begin') && request.method() === 'POST') beginRequests += 1;
  });
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Accounts' }).click();
  await page.getByRole('button', { name: 'Edit account setup' }).click();
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByRole('button', { name: 'Review & sign in' }).click();
  await page.getByLabel('Phone number').fill('+15550000000');
  await page.getByRole('button', { name: 'Save settings & send code' }).click();

  await expect(page.locator('.wizard-error')).toContainText('telegram settings rejected for this test');
  await expect(page.getByLabel('Phone number')).toHaveValue('+15550000000');
  expect(beginRequests).toBe(0);
});

test('upload dialog confirms cancellation while the file is still uploading to the server', async ({ page }) => {
  await mockAdminApi(page);
  let releaseChunk = () => {};
  const chunkGate = new Promise<void>((resolve) => { releaseChunk = resolve; });

  await page.route('**/_admin/api/uploads/resumable', async (route) => {
    if (route.request().method() === 'POST') {
      return route.fulfill({ json: { id: 'reception-1', chunk_size: 1, received: 0 } });
    }
    return route.fallback();
  });
  await page.route('**/_admin/api/uploads/resumable/reception-1*', async (route) => {
    if (route.request().method() === 'PATCH') {
      await chunkGate;
      return route.fulfill({ json: { received: 1 } });
    }
    if (route.request().method() === 'DELETE') {
      return route.fulfill({ json: { ok: true } });
    }
    return route.fallback();
  });

  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Buckets' }).click();
  await page.getByRole('button', { name: /^release-test created/ }).click();
  await page.getByRole('button', { name: 'Upload' }).click();
  await page.locator('input[type="file"]').setInputFiles({ name: 'server-upload.txt', mimeType: 'text/plain', buffer: Buffer.from('x') });
  await page.getByRole('button', { name: 'Upload 1' }).click();
  await expect(page.getByText('Receiving')).toBeVisible();

  await page.getByRole('button', { name: 'Close upload dialog' }).click();
  await expect(page.getByRole('heading', { name: 'Cancel this upload?' })).toBeVisible();
  await page.getByRole('button', { name: 'Keep uploading' }).click();
  await expect(page.getByRole('heading', { name: 'Cancel this upload?' })).toBeHidden();

  await page.locator('.modal-backdrop').first().click({ position: { x: 5, y: 5 } });
  await expect(page.getByRole('heading', { name: 'Cancel this upload?' })).toBeVisible();
  await page.getByRole('button', { name: 'Cancel upload' }).click();
  releaseChunk();
  await expect(page.getByRole('heading', { name: 'Upload files' })).toBeHidden();
});

test('connection removal clears recovery attention items from the panel', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Accounts' }).click();
  await page.getByRole('button', { name: 'Remove connection' }).first().click();
  const checkboxRow = page.locator('.checkbox-row');
  const checkboxBox = await checkboxRow.locator('input').boundingBox();
  const checkboxTextBox = await checkboxRow.locator('span').boundingBox();
  expect(checkboxBox).not.toBeNull();
  expect(checkboxTextBox).not.toBeNull();
  if (!checkboxBox || !checkboxTextBox) throw new Error('delete-files checkbox did not render');
  expect(Math.abs((checkboxBox.y + checkboxBox.height / 2) - (checkboxTextBox.y + checkboxTextBox.height / 2))).toBeLessThanOrEqual(2);
  await page.getByLabel('Type the displayed account number').fill('+15551234567');
  await page.locator('.compact-modal').getByRole('button', { name: 'Remove connection' }).click();

  await page.getByRole('button', { name: 'Transfers' }).click();
  await expect(page.getByText('No transfers yet. Upload a file from Buckets to get started.')).toBeVisible();
  await page.getByRole('button', { name: 'Recovery' }).click();
  await expect(page.getByText('No missing or corrupted files were detected.')).toBeVisible();
  await expect(page.locator('.health')).toContainText('Telegram · needs reauth');
});
