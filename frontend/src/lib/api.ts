import type {
  BucketsState,
  ObjectsState,
  OverviewState,
  SessionState,
  UsersState
} from './types';

const API_PREFIX = '/_admin/api';

type CsrfHeaders = Record<string, string>;

function csrfHeaders(token?: string | null): CsrfHeaders {
  return token ? { 'X-CSRF-Token': token } : {};
}

async function requestJson<T>(
  path: string,
  csrf?: string | null,
  options: { method?: string; body?: unknown } = {}
): Promise<T> {
  const headers: Record<string, string> = {
    Accept: 'application/json',
    ...csrfHeaders(csrf)
  };
  if (options.body !== undefined) {
    headers['Content-Type'] = 'application/json';
  }
  const response = await fetch(`${API_PREFIX}${path}`, {
    method: options.method ?? 'GET',
    credentials: 'include',
    headers,
    body: options.body !== undefined ? JSON.stringify(options.body) : undefined
  });

  if (!response.ok) {
    let message = `request failed with ${response.status}`;
    try {
      const payload = (await response.json()) as { error?: string };
      if (payload?.error) message = payload.error;
    } catch {
      // keep HTTP status message
    }
    throw new Error(message);
  }

  if (response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

export function getSession() {
  return requestJson<SessionState>('/session');
}

export function login(username: string, password: string) {
  return requestJson<SessionState>('/session/login', null, {
    method: 'POST',
    body: { username, password }
  });
}

export function logout(csrf?: string | null) {
  return requestJson<SessionState>('/session/logout', csrf, { method: 'POST' });
}

export function refreshSession(csrf?: string | null) {
  return requestJson<SessionState>('/session/refresh', csrf, { method: 'POST' });
}

export function getOverview() {
  return requestJson<OverviewState>('/overview');
}

export function listUsers(csrf?: string | null) {
  return requestJson<UsersState>('/users', csrf);
}

export function createUser(
  csrf?: string | null,
  body?: { username: string; password: string; display_name?: string; role?: string }
) {
  return requestJson<{ ok?: boolean }>('/users', csrf, { method: 'POST', body });
}

export function deleteUser(csrf?: string | null, id?: string) {
  return requestJson<{ ok?: boolean }>(`/users/${id}`, csrf, { method: 'DELETE' });
}

export function listBuckets(csrf?: string | null) {
  return requestJson<BucketsState>('/buckets', csrf);
}

export function listObjects(
  csrf: string | null | undefined,
  bucket: string,
  prefix: string,
  delimiter = true
) {
  const qp = new URLSearchParams({ bucket, prefix });
  if (delimiter) qp.set('delimiter', '1');
  return requestJson<ObjectsState>(`/objects?${qp.toString()}`, csrf);
}

export function createFolder(csrf?: string | null, bucket = '', path = '') {
  return requestJson<{ ok?: boolean }>('/objects/folder', csrf, {
    method: 'POST',
    body: { bucket, path }
  });
}

export function removeObject(csrf?: string | null, bucket = '', key = '') {
  return requestJson<{ ok?: boolean }>('/objects/delete', csrf, {
    method: 'POST',
    body: { bucket, key }
  });
}
