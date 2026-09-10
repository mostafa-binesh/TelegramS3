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

async function mockAdminApi(
  page: import('@playwright/test').Page,
  options: { telegramSettingsFailure?: boolean } = {}
) {
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
    if (path === '/setup' && request.method() === 'GET') {
      return route.fulfill({ json: { setup_required: false } });
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
            telegram_proxy_mode: body.telegram_proxy_mode ?? 'auto'
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
    if (path === '/objects') return route.fulfill({ json: { prefix: '', folders: [], objects: [{ key: 'readme.txt', name: 'readme.txt', size: 12, last_modified: '2026-01-01T00:00:00Z' }] } });
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
  await expect(page.getByRole('heading', { name: 'Your storage connection, beautifully in sync.' })).toBeVisible();
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

    await page.getByRole('button', { name: 'Telegram settings' }).click();
    await expect(page.getByRole('heading', { name: 'Your storage connection, beautifully in sync.' })).toBeVisible();
    await expectNoHorizontalOverflow();
  }

  await page.setViewportSize({ width: 320, height: 900 });
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
  await page.getByRole('button', { name: 'release-test' }).click();

  await expect(page.getByText('readme.txt')).toBeVisible();
  expect(objectListRequests).toBe(1);

  await page.getByRole('button', { name: 'All buckets' }).click();
  await page.getByRole('button', { name: 'release-test' }).click();

  await expect(page.getByText('readme.txt')).toBeVisible();
  expect(objectListRequests).toBe(2);
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
  await page.getByRole('button', { name: 'Telegram settings' }).click();
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
  await page.getByLabel('Storage chat ID').fill('-1009876543210');
  await page.getByRole('button', { name: 'Continue' }).click();

  await expect(page.getByRole('heading', { name: 'Make the connection reliable' })).toBeVisible();
  await page.getByLabel('Connection mode').selectOption('socks5');
  await page.getByLabel('Proxy URL').fill('socks5://127.0.0.1:12334');
  await page.getByRole('button', { name: 'Back' }).click();
  await expect(page.getByLabel('Storage chat ID')).toHaveValue('-1009876543210');
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
  await page.getByRole('button', { name: 'Telegram settings' }).click();
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
  await page.getByRole('button', { name: 'Remove connection' }).first().click();
  await page.locator('.compact-modal').getByRole('button', { name: 'Remove connection' }).click();

  await page.getByRole('button', { name: 'Transfers' }).click();
  await expect(page.getByText('No transfers yet. Upload a file from Buckets to get started.')).toBeVisible();
  await page.getByRole('button', { name: 'Recovery' }).click();
  await expect(page.getByText('No missing or corrupted files were detected.')).toBeVisible();
});
