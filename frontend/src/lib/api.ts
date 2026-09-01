import type { BootstrapState, OverviewState, SessionState } from './types';

const API_PREFIX = '/_admin/api';

async function requestJson<T>(
  path: string,
  init: RequestInit = {}
): Promise<T> {
  const response = await fetch(`${API_PREFIX}${path}`, {
    credentials: 'include',
    headers: {
      Accept: 'application/json',
      ...(init.body ? { 'Content-Type': 'application/json' } : {}),
      ...(init.headers ?? {})
    },
    ...init
  });

  if (!response.ok) {
    let message = `request failed with ${response.status}`;
    try {
      const payload = (await response.json()) as { error?: string };
      if (payload?.error) {
        message = payload.error;
      }
    } catch {
      // Keep the HTTP status message.
    }
    throw new Error(message);
  }

  return (await response.json()) as T;
}

export function getSession() {
  return requestJson<SessionState>('/session');
}

export function login(bootstrapSecret: string) {
  return requestJson<SessionState>('/session/login', {
    method: 'POST',
    body: JSON.stringify({ bootstrap_secret: bootstrapSecret })
  });
}

export function logout(csrfToken: string) {
  return requestJson<SessionState>('/session/logout', {
    method: 'POST',
    headers: {
      'X-CSRF-Token': csrfToken
    }
  });
}

export function refreshSession(csrfToken: string) {
  return requestJson<SessionState>('/session/refresh', {
    method: 'POST',
    headers: {
      'X-CSRF-Token': csrfToken
    }
  });
}

export function getOverview() {
  return requestJson<OverviewState>('/overview');
}

export function getBootstrapStatus() {
  return requestJson<BootstrapState>('/bootstrap-status');
}

export function runOnboardingCheck(csrfToken: string) {
  return requestJson<BootstrapState>('/onboarding/check', {
    method: 'POST',
    headers: {
      'X-CSRF-Token': csrfToken
    }
  });
}
