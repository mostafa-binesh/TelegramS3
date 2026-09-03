import type {
  BucketsState,
  FileUploadResult,
  ObjectsState,
  OverviewState,
  SessionState,
  UsersState,
  WizardState
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

export function createBucket(csrf?: string | null, name = '') {
  return requestJson<{ name: string; created_at: string }>('/buckets', csrf, {
    method: 'POST',
    body: { name }
  });
}

export function deleteBucket(csrf?: string | null, name = '') {
  return requestJson<{ ok?: boolean }>(`/buckets/${encodeURIComponent(name)}`, csrf, {
    method: 'DELETE'
  });
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

/** Absolute path for a content download/upload targeted at the given object key. */
export function contentUrl(bucket: string, key: string) {
  const qp = new URLSearchParams({ bucket, key });
  return `${API_PREFIX}/objects/content?${qp.toString()}`;
}

/**
 * Upload raw file bytes to a bucket key with per-second progress reporting.
 * The body is sent verbatim (not JSON) and the CSRF token rides the header.
 */
export async function uploadObject(
  bucket: string,
  key: string,
  file: Blob,
  csrf?: string | null,
  onProgress?: (sent: number, total: number) => void
): Promise<FileUploadResult> {
  const total = file.size;
  return new Promise<FileUploadResult>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', contentUrl(bucket, key));
    xhr.responseType = 'json';
    xhr.withCredentials = true;
    if (csrf) xhr.setRequestHeader('X-CSRF-Token', csrf);
    if (onProgress) {
      xhr.upload.onprogress = (event) => {
        if (event.lengthComputable) onProgress(event.loaded, total);
      };
    }
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        const body = xhr.response as Partial<FileUploadResult> | null;
        if (body && typeof body.size === 'number' && body.etag && body.version_id) {
          resolve(body as FileUploadResult);
        } else {
          reject(new Error('upload succeeded but returned an unexpected payload'));
        }
      } else {
        let message = `upload failed with ${xhr.status}`;
        try {
          const body = xhr.response as { error?: string };
          if (body?.error) message = body.error;
        } catch {
          // fall back to HTTP status message
        }
        reject(new Error(message));
      }
    };
    xhr.onerror = () => reject(new Error('upload request failed'));
    xhr.onabort = () => reject(new Error('upload aborted'));
    xhr.send(file);
  });
}

export function getWizardState(csrf?: string | null) {
  return requestJson<WizardState>('/telegram/wizard/state', csrf);
}

export function wizardBegin(phone: string | undefined, csrf?: string | null) {
  return requestJson<WizardState>('/telegram/wizard/begin', csrf, {
    method: 'POST',
    body: phone === undefined ? {} : { phone }
  });
}

export function wizardSubmitCode(code: string, csrf?: string | null) {
  return requestJson<WizardState>('/telegram/wizard/submit-code', csrf, {
    method: 'POST',
    body: { code }
  });
}

export function wizardSubmitPassword(password: string, csrf?: string | null) {
  return requestJson<WizardState>('/telegram/wizard/submit-password', csrf, {
    method: 'POST',
    body: { password }
  });
}

export function wizardCancel(csrf?: string | null) {
  return requestJson<{ ok: boolean }>('/telegram/wizard/cancel', csrf, { method: 'POST' });
}
