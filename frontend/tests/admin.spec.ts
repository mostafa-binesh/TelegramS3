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
    recovery_required_objects: 0
  },
  recovery: {
    issue_count: 0,
    unacknowledged_count: 0,
    scan_ok: true,
    issues: []
  },
  telegram: {
    session_state: 'authorized',
    connection_state: 'connected',
    detail: 'mock storage chat reachable',
    storage_chat_id: '-1001234567890'
  },
  checks: []
};

async function mockAdminApi(page: import('@playwright/test').Page) {
  let loggedIn = false;
  let recoveryJobVisible = true;
  let connectionRemoved = false;
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
    if (!loggedIn) return route.fulfill({ status: 401, json: { error: 'unauthorized' } });
    if (path === '/overview') {
      const clearedOverview = {
        ...overview,
        storage: { ...overview.storage, recovery_required_objects: 0 },
        recovery: { ...overview.recovery, issue_count: 0, unacknowledged_count: 0, issues: [] },
        telegram: { ...overview.telegram, connection_state: 'needs_reauth', detail: 'Telegram storage is not connected' }
      };
      return route.fulfill({ json: connectionRemoved ? clearedOverview : overview });
    }
    if (path === '/telegram/disconnect' && request.method() === 'POST') {
      connectionRemoved = true;
      recoveryJobVisible = false;
      return route.fulfill({ status: 202, json: { ok: true, job: { id: 'removal-1', state: 'pending', delete_uploaded_files: false }, message: 'Connection removed.' } });
    }
    if (path === '/telegram/settings') {
      return route.fulfill({
        json: {
          settings: {
            telegram_api_id: '12345',
            telegram_api_hash: 'hash',
            telegram_storage_chat_id: '-1001234567890',
            telegram_proxy_url: '',
            telegram_proxy_username: '',
            telegram_proxy_password: '',
            telegram_proxy_mode: 'auto'
          }
        }
      });
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
    if (path === '/buckets') return route.fulfill({ json: { buckets: [{ name: 'release-test', created_at: '2026-01-01T00:00:00Z' }] } });
    if (path === '/objects') return route.fulfill({ json: { prefix: '', folders: [], objects: [] } });
    if (path === '/users') return route.fulfill({ json: { users: [user] } });
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
}

test('guest is gated, authenticated navigation works, and logout revokes the session', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('/');

  await expect(page.getByRole('heading', { name: 'Sign in to manage storage' })).toBeVisible();
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();

  await expect(page.getByRole('heading', { name: 'Storage at a glance' })).toBeVisible();
  await page.getByRole('button', { name: 'Telegram settings' }).click();
  await expect(page.getByRole('heading', { name: 'Telegram storage is connected' })).toBeVisible();
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: 'Sign in to manage storage' })).toBeVisible();
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

test('telegram wizard advances each step when Enter is pressed', async ({ page }) => {
  await mockAdminApi(page);
  await page.goto('/');
  await page.getByLabel('Username').fill('admin');
  await page.getByLabel('Password').fill('correct-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.getByRole('button', { name: 'Telegram settings' }).click();
  await page.getByRole('button', { name: 'Refresh Telegram login' }).click();

  await page.getByLabel('Phone (international format)').fill('+15550000000');
  await page.getByLabel('Phone (international format)').press('Enter');
  await expect(page.getByLabel('Confirmation code')).toBeVisible();

  await page.getByLabel('Confirmation code').fill('123456');
  await page.getByLabel('Confirmation code').press('Enter');
  await expect(page.getByLabel('Cloud password')).toBeVisible();

  await page.getByLabel('Cloud password').fill('test-cloud-password');
  await page.getByLabel('Cloud password').press('Enter');
  await expect(page.getByRole('heading', { name: 'Set up Telegram storage access' })).toBeHidden();
  await expect(page.getByText('Telegram account authorized.')).toBeVisible();
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
  await page.getByRole('button', { name: 'release-test' }).click();
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
  await page.getByRole('button', { name: 'Telegram settings' }).click();
  await page.getByRole('button', { name: 'Remove current connection' }).click();
  await page.getByRole('button', { name: 'Remove connection' }).click();

  await page.getByRole('button', { name: 'Transfers' }).click();
  await expect(page.getByText('No transfers yet. Upload a file from Buckets to get started.')).toBeVisible();
  await page.getByRole('button', { name: 'Recovery' }).click();
  await expect(page.getByText('No missing or corrupted files were detected.')).toBeVisible();
});
