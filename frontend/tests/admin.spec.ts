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
    if (path === '/overview') return route.fulfill({ json: overview });
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
    if (path === '/buckets') return route.fulfill({ json: { buckets: [{ name: 'release-test', created_at: '2026-01-01T00:00:00Z' }] } });
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
