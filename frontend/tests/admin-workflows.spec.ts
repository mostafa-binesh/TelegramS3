import { expect, test, type Page } from '@playwright/test';

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
    committed_objects: 2,
    active_objects: 2,
    staged_objects: 0,
    recovery_markers: 0,
    chunk_size: 1_048_576,
    recovery_required_objects: 0
  },
  recovery: { issue_count: 0, unacknowledged_count: 0, scan_ok: true, issues: [] },
  telegram: {
    session_state: 'authorized',
    connection_state: 'connected',
    detail: 'mock storage chat reachable',
    storage_chat_id: '-1001234567890'
  },
  checks: []
};

type MockOptions = {
  shareFailure?: boolean;
  storageFailure?: boolean;
  recoveryIssue?: Record<string, unknown>;
};

async function mockAdminApi(page: Page, options: MockOptions = {}) {
  let loggedIn = false;
  let chunkSize = overview.storage.chunk_size;
  let connectionRemoved = false;
  let createdFolders = new Set(['docs']);
  const deletedKeys = new Set<string>();
  const buckets = [{ name: 'release-test', created_at: '2026-01-01T00:00:00Z' }];
  const users = [user];
  let recoveryIssue = options.recoveryIssue ? { ...options.recoveryIssue, acknowledged_at: null, acknowledged_by: null } : null;

  await page.route('**/_admin/api/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname.replace('/_admin/api', '');

    if (path === '/session' && request.method() === 'GET') {
      return route.fulfill({ json: loggedIn ? authenticated : { authenticated: false } });
    }
    if (path === '/session/login' && request.method() === 'POST') {
      loggedIn = true;
      return route.fulfill({ json: authenticated });
    }
    if (path === '/session/logout' && request.method() === 'POST') {
      loggedIn = false;
      return route.fulfill({ json: { authenticated: false } });
    }
    if (path === '/setup' && request.method() === 'GET') {
      return route.fulfill({ json: { setup_required: false } });
    }
    if (!loggedIn) return route.fulfill({ status: 401, json: { error: 'unauthorized' } });

    if (path === '/overview' && request.method() === 'GET') {
      const issue = recoveryIssue ? [recoveryIssue] : [];
      return route.fulfill({
        json: {
          ...overview,
          storage: { ...overview.storage, buckets: buckets.length, committed_objects: 2 - deletedKeys.size, active_objects: 2 - deletedKeys.size, chunk_size: chunkSize },
          recovery: { ...overview.recovery, issue_count: issue.filter((item) => !item.acknowledged_at).length, unacknowledged_count: issue.filter((item) => !item.acknowledged_at).length, issues: issue },
          telegram: connectionRemoved ? { ...overview.telegram, connection_state: 'needs_reauth', detail: 'Telegram storage is not connected' } : overview.telegram
        }
      });
    }

    if (path === '/telegram/settings' && request.method() === 'GET') {
      return route.fulfill({ json: { settings: { telegram_api_id: '12345', telegram_api_hash: 'hash', telegram_storage_chat_id: '-1001234567890', telegram_proxy_url: '', telegram_proxy_username: '', telegram_proxy_password: '', telegram_proxy_mode: 'auto', telegram_account_phone: '+15551234567' } } });
    }
    if (path === '/telegram/storage-settings' && request.method() === 'GET') {
      return route.fulfill({ json: { chunk_size: chunkSize, min_chunk_size: 1, max_chunk_size: 2_000_000_000, source: 'database' } });
    }
    if (path === '/telegram/storage-settings' && request.method() === 'POST') {
      if (options.storageFailure) return route.fulfill({ status: 400, json: { error: 'storage settings rejected for this test' } });
      chunkSize = (request.postDataJSON() as { chunk_size: number }).chunk_size;
      return route.fulfill({ json: { chunk_size: chunkSize, min_chunk_size: 1, max_chunk_size: 2_000_000_000, source: 'database' } });
    }
    if (path === '/telegram/disconnect' && request.method() === 'POST') {
      connectionRemoved = true;
      return route.fulfill({ status: 202, json: { ok: true, job: { id: 'removal-1', state: 'pending', delete_uploaded_files: false }, message: 'Connection removed.' } });
    }

    if (path === '/buckets' && request.method() === 'GET') return route.fulfill({ json: { buckets } });
    if (path === '/buckets' && request.method() === 'POST') {
      const body = request.postDataJSON() as { name: string };
      buckets.push({ name: body.name, created_at: '2026-01-01T00:00:00Z' });
      return route.fulfill({ json: buckets[buckets.length - 1] });
    }
    if (path.startsWith('/buckets/') && request.method() === 'DELETE') {
      const name = decodeURIComponent(path.slice('/buckets/'.length));
      const index = buckets.findIndex((bucket) => bucket.name === name);
      if (index >= 0) buckets.splice(index, 1);
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/objects' && request.method() === 'GET') {
      const prefix = new URL(request.url()).searchParams.get('prefix') ?? '';
      const objects = prefix
        ? [{ key: 'docs/report.txt', name: 'report.txt', size: 24, last_modified: '2026-01-01T00:00:00Z' }]
        : [
            { key: 'readme.txt', name: 'readme.txt', size: 12, last_modified: '2026-01-01T00:00:00Z' },
            { key: 'archive.bin', name: 'archive.bin', size: 28, last_modified: '2026-01-01T00:00:00Z' }
          ];
      return route.fulfill({ json: { prefix, folders: prefix ? [] : [...createdFolders].filter((folder) => !deletedKeys.has(`${folder}/`)), objects: objects.filter((object) => !deletedKeys.has(object.key)) } });
    }
    if (path === '/objects/share' && request.method() === 'POST') {
      if (options.shareFailure) return route.fulfill({ status: 400, json: { error: 'share creation failed for this test' } });
      const body = request.postDataJSON() as { expires_in_seconds?: number };
      return route.fulfill({ status: 201, json: { url: '/_public/mock-share-token', expires_at: body.expires_in_seconds ? '2026-01-01T01:00:00Z' : null } });
    }
    if (path === '/objects/delete' && request.method() === 'POST') {
      deletedKeys.add((request.postDataJSON() as { key: string }).key);
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/objects/folder' && request.method() === 'POST') {
      const pathName = (request.postDataJSON() as { path: string }).path;
      createdFolders = new Set([...createdFolders, pathName.replace(/\/$/, '')]);
      return route.fulfill({ json: { ok: true } });
    }
    if (path === '/users' && request.method() === 'GET') return route.fulfill({ json: { users } });
    if (path === '/users' && request.method() === 'POST') {
      const body = request.postDataJSON() as { username: string; display_name?: string; role?: string };
      users.push({ id: `user-${users.length + 1}`, username: body.username, display_name: body.display_name ?? '', role: body.role ?? 'admin', disabled: false });
      return route.fulfill({ json: { ok: true } });
    }
    if (path.startsWith('/users/') && request.method() === 'DELETE') return route.fulfill({ json: { ok: true } });
    if (path === '/recovery/acknowledge' && request.method() === 'POST') {
      if (recoveryIssue) recoveryIssue = { ...recoveryIssue, acknowledged_at: '2026-01-01T01:00:00Z', acknowledged_by: 'admin' };
      return route.fulfill({ json: { ok: true, acknowledged_count: 1, unacknowledged_count: 0 } });
    }
    if (path === '/recovery/unacknowledge' && request.method() === 'POST') {
      if (recoveryIssue) recoveryIssue = { ...recoveryIssue, acknowledged_at: null, acknowledged_by: null };
      return route.fulfill({ json: { ok: true, acknowledged_count: 0, unacknowledged_count: 1 } });
    }
    if (path === '/jobs' && request.method() === 'GET') return route.fulfill({ json: { jobs: [], next_offset: null } });
    if (path === '/jobs/job-upload' && request.method() === 'GET') return route.fulfill({ json: { id: 'job-upload', object_id: 'object-upload', operation_id: 'operation-upload', bucket: 'release-test', key: 'upload.txt', state: 'completed', bytes: 1, chunks_done: 1, chunks_total: 1, attempts: 1, next_retry: 0, created_at: 1_767_000_000, updated_at: 1_767_000_001 } });
    if (path === '/uploads/resumable' && request.method() === 'POST') return route.fulfill({ json: { id: 'reception-1', chunk_size: 1, received: 0 } });
    if (path === '/uploads/resumable/reception-1' && request.method() === 'PATCH') return route.fulfill({ json: { received: 1 } });
    if (path === '/uploads/resumable/reception-1/complete' && request.method() === 'POST') return route.fulfill({ json: { job_id: 'job-upload' } });
    if (path === '/uploads/resumable/reception-1' && request.method() === 'DELETE') return route.fulfill({ json: { ok: true } });
    return route.fulfill({ json: {} });
  });
}

async function signIn(page: Page) {
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'Storage at a glance' })).toBeVisible();
}

async function openBucket(page: Page) {
  await page.getByRole('button', { name: 'Buckets' }).click();
  await page.getByRole('button', { name: /^release-test created/ }).click();
  await expect(page.getByRole('heading', { name: 'Bucket / release-test' })).toBeVisible();
}

test('share modal uses an expiry preset, sends the correct payload, displays, and opens the public URL', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await openBucket(page);

  const shareButton = page.getByRole('button', { name: 'Share readme.txt' });
  await expect(shareButton).toHaveAttribute('title', 'Share readme.txt');
  await shareButton.click();
  await expect(page.getByRole('heading', { name: 'Share readme.txt' })).toBeVisible();
  await page.getByRole('button', { name: '1 day', exact: true }).click();
  await expect(page.getByLabel('Link lifetime')).toHaveValue('86400');

  const request = page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/objects/share') && candidate.method() === 'POST');
  await page.getByRole('button', { name: 'Create share link' }).click();
  const shareRequest = await request;
  expect(shareRequest.postDataJSON()).toMatchObject({ bucket: 'release-test', key: 'readme.txt', expires_in_seconds: 86400 });
  await expect(page.getByLabel('Public share URL')).toHaveValue(/\/_public\/mock-share-token$/);
  await expect(page.getByText('Link ready')).toBeVisible();

  const publicPage = await page.context().newPage();
  await publicPage.route('**/_public/mock-share-token', (route) => route.fulfill({ status: 200, body: 'shared bytes', headers: { 'content-disposition': 'attachment; filename="readme.txt"' } }));
  const publicUrl = await page.getByLabel('Public share URL').inputValue();
  const downloadPromise = publicPage.waitForEvent('download');
  await publicPage.goto(publicUrl).catch(() => undefined);
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toBe('readme.txt');
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream ?? []) chunks.push(Buffer.from(chunk));
  expect(Buffer.concat(chunks).toString()).toBe('shared bytes');
  await publicPage.close();
});

for (const seconds of [10, 30]) {
  test(`share modal accepts an arbitrary ${seconds}-second lifetime`, async ({ page }) => {
    await mockAdminApi(page);
    await signIn(page);
    await openBucket(page);
    await page.getByRole('button', { name: 'Share readme.txt' }).click();
    await expect(page.locator('.duration-unit')).toHaveText('seconds');
    await page.getByLabel('Link lifetime').fill(String(seconds));

    const [shareRequest] = await Promise.all([
      page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/objects/share') && candidate.method() === 'POST'),
      page.getByRole('button', { name: 'Create share link' }).click()
    ]);
    expect(shareRequest.postDataJSON()).toMatchObject({ bucket: 'release-test', key: 'readme.txt', expires_in_seconds: seconds });
    await expect(page.getByText('Link ready')).toBeVisible();
  });
}

test('share modal supports a never-expire link and surfaces creation errors', async ({ page }) => {
  await mockAdminApi(page, { shareFailure: true });
  await signIn(page);
  await openBucket(page);
  await page.getByRole('button', { name: 'Share archive.bin' }).click();
  await page.getByRole('button', { name: 'Never', exact: true }).click();
  const request = page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/objects/share') && candidate.method() === 'POST');
  await page.getByRole('button', { name: 'Create share link' }).click();
  expect((await request).postDataJSON()).not.toHaveProperty('expires_in_seconds');
  await expect(page.getByRole('alert')).toContainText('share creation failed for this test');
  await expect(page.getByRole('heading', { name: 'Share archive.bin' })).toBeVisible();
});

test('object and folder deletion require confirmation, while cancel leaves the listing unchanged', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await openBucket(page);

  await page.getByRole('button', { name: 'Delete readme.txt' }).click();
  await expect(page.getByRole('heading', { name: 'Delete object?' })).toBeVisible();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByText('readme.txt')).toBeVisible();

  const objectDelete = page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/objects/delete') && candidate.method() === 'POST');
  await page.getByRole('button', { name: 'Delete readme.txt' }).click();
  await page.locator('.compact-modal').getByRole('button', { name: 'Delete', exact: true }).click();
  expect((await objectDelete).postDataJSON()).toMatchObject({ bucket: 'release-test', key: 'readme.txt' });
  await expect(page.getByText('readme.txt')).toBeHidden();

  const folderDelete = page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/objects/delete') && candidate.method() === 'POST');
  await page.getByRole('button', { name: 'Delete folder docs' }).click();
  await expect(page.getByRole('heading', { name: 'Delete folder?' })).toBeVisible();
  await page.locator('.compact-modal').getByRole('button', { name: 'Delete', exact: true }).click();
  expect((await folderDelete).postDataJSON()).toMatchObject({ bucket: 'release-test', key: 'docs/' });
});

test('bucket toolbar creates a folder and folder navigation updates the breadcrumb', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await openBucket(page);
  await expect(page.getByRole('button', { name: 'Refresh' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'New folder' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Upload' })).toBeVisible();

  await page.getByRole('button', { name: 'docs/' }).click();
  await expect(page.getByRole('heading', { name: 'Bucket / release-test / docs' })).toBeVisible();
  await expect(page.getByText('report.txt')).toBeVisible();
  await page.getByRole('button', { name: 'Back to parent folder' }).click();
  await expect(page.getByRole('heading', { name: 'Bucket / release-test' })).toBeVisible();

  await page.getByRole('button', { name: 'New folder' }).click();
  await expect(page.getByRole('heading', { name: 'New folder' })).toBeVisible();
  await page.getByLabel('Folder name').fill('archive');
  await expect(page.getByText('Created inside the bucket root.')).toBeVisible();
  const folderRequest = page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/objects/folder') && candidate.method() === 'POST');
  await page.getByRole('button', { name: 'Create folder' }).click();
  expect((await folderRequest).postDataJSON()).toMatchObject({ bucket: 'release-test', path: 'archive/' });
  await expect(page.getByText('Folder created.')).toBeVisible();
});

test('bucket creation, bucket deletion, and multi-selection deletion use guarded actions', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await page.getByRole('button', { name: 'Buckets' }).click();

  await page.getByRole('button', { name: /Create bucket/ }).click();
  await page.getByLabel('Bucket name').fill('new-bucket');
  const [createRequest] = await Promise.all([
    page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/buckets') && candidate.method() === 'POST'),
    page.locator('.compact-modal').getByRole('button', { name: 'Create bucket', exact: true }).click()
  ]);
  expect(createRequest.postDataJSON()).toMatchObject({ name: 'new-bucket' });
  await expect(page.getByRole('button', { name: /^new-bucket created/ })).toBeVisible();

  const bucketDelete = page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/buckets/new-bucket') && candidate.method() === 'DELETE');
  await page.getByRole('button', { name: 'Delete bucket new-bucket' }).click();
  await expect(page.getByRole('heading', { name: 'Delete bucket?' })).toBeVisible();
  await expect(page.getByText('The bucket must already be empty.')).toBeVisible();
  await page.locator('.compact-modal').getByRole('button', { name: 'Delete', exact: true }).click();
  await bucketDelete;
  await expect(page.getByRole('button', { name: /^new-bucket created/ })).toBeHidden();

  await page.getByRole('button', { name: /^release-test created/ }).click();
  await page.getByLabel('Select readme.txt').check();
  await page.getByLabel('Select archive.bin').check();
  await page.locator('.selection-bar').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Delete selected items?' })).toBeVisible();
  await page.locator('.compact-modal').getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByText('Selected items deleted.')).toBeVisible();
  await expect(page.getByText('readme.txt')).toBeHidden();
  await expect(page.getByText('archive.bin')).toBeHidden();
});

test('move modal browses destinations and moves the selected object', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await openBucket(page);
  await page.getByLabel('Select readme.txt').check();
  await page.locator('.selection-bar').getByRole('button', { name: /Move/ }).click();
  await expect(page.getByRole('heading', { name: 'Choose destination' })).toBeVisible();

  const moveBrowser = page.locator('.move-browser');
  await moveBrowser.getByRole('region', { name: 'Destination buckets' }).getByRole('button', { name: 'release-test', exact: true }).click();
  await moveBrowser.getByRole('button', { name: 'docs/' }).click();
  await expect(moveBrowser.getByText('Move here:')).toBeVisible();
  await expect(moveBrowser).toContainText('release-test/docs');

  const deleteRequest = page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/objects/delete') && candidate.method() === 'POST');
  await moveBrowser.getByRole('button', { name: 'Move files' }).click();
  expect((await deleteRequest).postDataJSON()).toMatchObject({ bucket: 'release-test', key: 'readme.txt' });
  await expect(page.getByText('Selected items moved.')).toBeVisible();
});

test('upload modal sends the selected object expiry through the resumable upload flow', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await openBucket(page);
  await page.getByRole('button', { name: 'Upload' }).click();
  await page.locator('input[type="file"]').setInputFiles({ name: 'upload.txt', mimeType: 'text/plain', buffer: Buffer.from('x') });
  const expiryInput = page.getByLabel('Object expiry in seconds');
  await page.getByRole('button', { name: '1 hour', exact: true }).click();
  await expect(expiryInput).toHaveValue('3600');
  await expiryInput.fill('30');
  const [beginRequest] = await Promise.all([
    page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/uploads/resumable') && candidate.method() === 'POST'),
    page.getByRole('button', { name: 'Upload 1' }).click()
  ]);
  expect(beginRequest.postDataJSON()).toMatchObject({ bucket: 'release-test', key: 'upload.txt', expires_in_seconds: 30 });
  await expect(page.getByText('completed')).toBeVisible();
});

test('storage policy tab loads, applies MiB to bytes, and reports a save failure', async ({ page }) => {
  await mockAdminApi(page, { storageFailure: true });
  await signIn(page);
  await page.getByRole('button', { name: 'Telegram settings' }).click();
  await page.getByRole('tab', { name: /Storage policy/ }).click();
  await expect(page.getByRole('heading', { name: 'Give every upload the right-sized runway.' })).toBeVisible();
  await expect(page.getByLabel('New upload chunk size')).toHaveValue('1');
  await page.getByRole('button', { name: '8 MiB', exact: true }).click();
  await expect(page.getByLabel('New upload chunk size')).toHaveValue('8');
  const [saveRequest] = await Promise.all([
    page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/telegram/storage-settings') && candidate.method() === 'POST'),
    page.getByRole('button', { name: 'Apply storage policy' }).click()
  ]);
  expect(saveRequest.postDataJSON()).toMatchObject({ chunk_size: 8 * 1_048_576 });
  await expect(page.getByRole('alert')).toContainText('storage settings rejected for this test');
});

test('storage policy success is reflected after leaving and returning to the tab', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await page.getByRole('button', { name: 'Telegram settings' }).click();
  await page.getByRole('tab', { name: /Storage policy/ }).click();
  await page.getByRole('button', { name: '8 MiB', exact: true }).click();
  await Promise.all([
    page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/telegram/storage-settings') && candidate.method() === 'POST'),
    page.getByRole('button', { name: 'Apply storage policy' }).click()
  ]);
  await expect(page.locator('p.storage-message[role="status"]')).toContainText('Storage policy updated.');
  await page.getByRole('tab', { name: /Connection/ }).click();
  await page.getByRole('tab', { name: /Storage policy/ }).click();
  await expect(page.getByText('8.00 MiB now')).toBeVisible();
});

test('connection removal shows the account and stays disabled until its exact number is entered', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await page.getByRole('button', { name: 'Telegram settings' }).click();
  await page.getByRole('button', { name: 'Remove connection' }).first().click();
  const removeButton = page.locator('.compact-modal').getByRole('button', { name: 'Remove connection' });
  await expect(removeButton).toBeDisabled();
  await expect(page.locator('.account-confirmation')).toContainText('+15551234567');
  await page.getByLabel('Type the displayed account number').fill('+1 (555) 123-4567');
  await expect(removeButton).toBeDisabled();
  await page.getByLabel('Type the displayed account number').fill('+15551234567');
  await expect(removeButton).toBeEnabled();
  const [removeRequest] = await Promise.all([
    page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/telegram/disconnect') && candidate.method() === 'POST'),
    removeButton.click()
  ]);
  expect(removeRequest.postDataJSON()).toMatchObject({ delete_uploaded_files: false, phone_confirmation: '+15551234567' });
  await expect(page.getByText('Connection removed.')).toBeVisible();
});

test('recovery issue can be acknowledged and restored from the UI', async ({ page }) => {
  await mockAdminApi(page, { recoveryIssue: { id: 'issue-1', kind: 'missing_chunk', path: 'release-test/readme.txt', summary: 'Missing chunk', details: ['chunk 0 is unavailable'] } });
  await signIn(page);
  await page.getByRole('button', { name: 'Recovery' }).click();
  await expect(page.getByRole('heading', { name: '1 file needs attention' })).toBeVisible();
  await page.getByText('release-test/readme.txt').click();
  await page.getByRole('button', { name: 'Acknowledge' }).click();
  await expect(page.getByText('Every detected issue has been acknowledged.')).toBeVisible();
  await page.getByText('Acknowledged (1)').click();
  await page.locator('details.is-acknowledged summary').click();
  await page.getByRole('button', { name: 'Restore to list' }).click();
  await expect(page.getByRole('heading', { name: '1 file needs attention' })).toBeVisible();
});

test('operator deletion also uses the confirmation modal', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await page.getByRole('button', { name: 'Operators' }).click();
  await page.getByRole('button', { name: 'Remove operator admin' }).click();
  await expect(page.getByRole('heading', { name: 'Delete operator?' })).toBeVisible();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.locator('.kv-table tbody tr')).toContainText('admin');

  const deleteRequest = page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/users/user-1') && candidate.method() === 'DELETE');
  await page.getByRole('button', { name: 'Remove operator admin' }).click();
  await page.locator('.compact-modal').getByRole('button', { name: 'Delete', exact: true }).click();
  await deleteRequest;
});

test('superadmin can add an operator through the modal', async ({ page }) => {
  await mockAdminApi(page);
  await signIn(page);
  await page.getByRole('button', { name: 'Operators' }).click();
  await page.getByRole('button', { name: 'Add operator' }).click();
  await page.getByLabel('Username').last().fill('backup-admin');
  await page.getByLabel('Display name').fill('Backup Admin');
  await page.getByLabel('Password (12+ chars)').fill('long-enough-password');
  await page.getByLabel('Role').selectOption('admin');

  const [createRequest] = await Promise.all([
    page.waitForRequest((candidate) => candidate.url().endsWith('/_admin/api/users') && candidate.method() === 'POST'),
    page.locator('.modal-card').getByRole('button', { name: 'Add operator', exact: true }).click()
  ]);
  expect(createRequest.postDataJSON()).toMatchObject({ username: 'backup-admin', display_name: 'Backup Admin', password: 'long-enough-password', role: 'admin' });
  await expect(page.locator('.kv-table tbody')).toContainText('backup-admin');
});
