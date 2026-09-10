import type {
  TransferJob,
  BucketsState,
  ObjectsState,
  OverviewState,
  SessionState,
  TelegramSettingsState,
  UsersState,
  WizardState
} from './types';

const API_PREFIX = '/_admin/api';

type CsrfHeaders = Record<string, string>;

class ApiError extends Error {
  constructor(message: string, readonly status: number) {
    super(message);
    this.name = 'ApiError';
  }
}

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
    throw new ApiError(message, response.status);
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

export function getTelegramSettings() {
  return requestJson<TelegramSettingsState>('/telegram/settings');
}

export function saveTelegramSettings(
  csrf?: string | null,
  body?: Partial<TelegramSettingsState['settings']>
) {
  return requestJson<TelegramSettingsState>('/telegram/settings', csrf, {
    method: 'POST',
    body
  });
}

interface RecoveryAckResult {
  ok: boolean;
  acknowledged_count: number;
  unacknowledged_count: number;
}

/** Stop counting the given recovery issues on the Overview. */
export function acknowledgeRecovery(csrf: string | null | undefined, ids: string[]) {
  return requestJson<RecoveryAckResult>('/recovery/acknowledge', csrf, {
    method: 'POST',
    body: { ids }
  });
}

/** Bring previously acknowledged recovery issues back into the count. */
export function unacknowledgeRecovery(csrf: string | null | undefined, ids: string[]) {
  return requestJson<RecoveryAckResult>('/recovery/unacknowledge', csrf, {
    method: 'POST',
    body: { ids }
  });
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

export function createShareLink(
  csrf: string | null | undefined,
  bucket: string,
  key: string,
  expiresInSeconds?: number | null
) {
  return requestJson<{ url: string; expires_at?: string | null }>('/objects/share', csrf, {
    method: 'POST',
    body: { bucket, key, expires_in_seconds: expiresInSeconds || undefined }
  });
}

/** Write an already-read object body and wait for it to become committed. */
export async function putObjectContent(
  bucket: string,
  key: string,
  file: Blob,
  csrf?: string | null
) {
  const response = await fetch(contentUrl(bucket, key), {
    method: 'POST',
    credentials: 'include',
    headers: {
      'Content-Type': file.type || 'application/octet-stream',
      ...csrfHeaders(csrf)
    },
    body: file
  });
  if (!response.ok) {
    let message = `request failed with ${response.status}`;
    try {
      const payload = (await response.json()) as { error?: string };
      if (payload?.error) message = payload.error;
    } catch {
      // keep HTTP status message
    }
    throw new ApiError(message, response.status);
  }
  return (await response.json()) as { size: number; etag: string; version_id: string };
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
): Promise<{job_id:string}> {
  const total = file.size;
  const send = () => new Promise<{job_id:string}>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', `${API_PREFIX}/uploads?${new URLSearchParams({bucket,key})}`);
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
        const body = xhr.response as Partial<{job_id:string}> | null;
        if (body && body.job_id) {
          resolve(body as {job_id:string});
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
  let lastError: unknown;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try { return await send(); }
    catch (cause) {
      lastError = cause;
      if (attempt === 3) break;
      await new Promise((resolve) => setTimeout(resolve, 700 * 2 ** attempt));
    }
  }
  throw lastError instanceof Error ? lastError : new Error('upload request failed');
}

export function removeTelegramConnection(
  csrf: string | null | undefined,
  deleteUploadedFiles: boolean
) {
  return requestJson<{ ok: boolean; job: { id: string; state: string; delete_uploaded_files: boolean }; message: string }>(
    '/telegram/disconnect',
    csrf,
    { method: 'POST', body: { delete_uploaded_files: deleteUploadedFiles } }
  );
}

export interface ResumableUploadSession {
  id: string;
  chunk_size: number;
  received: number;
}

export interface ResumableUploadOptions {
  signal?: AbortSignal;
  receptionId?: string;
  waitUntilResumed?: () => Promise<void>;
  onReception?: (id: string) => void;
  expiresInSeconds?: number | null;
}

export function beginResumableUpload(
  bucket: string,
  key: string,
  contentType: string,
  csrf?: string | null,
  expiresInSeconds?: number | null
) {
  return requestJson<ResumableUploadSession>('/uploads/resumable', csrf, {
    method: 'POST',
    body: { bucket, key, content_type: contentType, expires_in_seconds: expiresInSeconds || undefined }
  });
}

export function resumableStatus(id: string, csrf?: string | null) {
  return requestJson<ResumableUploadSession>(`/uploads/resumable/${encodeURIComponent(id)}`, csrf);
}

export function completeResumableUpload(id: string, csrf?: string | null) {
  return requestJson<{ job_id: string; job: TransferJob }>(
    `/uploads/resumable/${encodeURIComponent(id)}/complete`,
    csrf,
    { method: 'POST' }
  );
}

export function abortResumableUpload(id: string, csrf?: string | null) {
  return requestJson<{ ok: boolean }>(`/uploads/resumable/${encodeURIComponent(id)}`, csrf, {
    method: 'DELETE'
  });
}

export function sendResumableChunk(
  id: string,
  offset: number,
  bytes: Blob,
  final: boolean,
  csrf?: string | null,
  signal?: AbortSignal,
  onProgress?: (sent: number, total: number) => void
) {
  return new Promise<{ received: number }>((resolve, reject) => {
    const query = new URLSearchParams({ offset: String(offset), final: final ? '1' : '0' });
    const xhr = new XMLHttpRequest();
    let settled = false;
    const cleanup = () => signal?.removeEventListener('abort', abort);
    const abort = () => xhr.abort();
    xhr.open('PATCH', `${API_PREFIX}/uploads/resumable/${encodeURIComponent(id)}?${query}`);
    xhr.responseType = 'json';
    xhr.withCredentials = true;
    xhr.setRequestHeader('Content-Type', 'application/octet-stream');
    if (csrf) xhr.setRequestHeader('X-CSRF-Token', csrf);
    if (signal) {
      if (signal.aborted) { xhr.abort(); return; }
      signal.addEventListener('abort', abort, { once: true });
    }
    if (onProgress) xhr.upload.onprogress = (event) => {
      if (event.lengthComputable) onProgress(event.loaded, bytes.size);
    };
    xhr.onload = () => {
      cleanup();
      if (xhr.status >= 200 && xhr.status < 300 && xhr.response?.received !== undefined) {
        settled = true;
        resolve(xhr.response as { received: number });
        return;
      }
      let message = `upload chunk failed with ${xhr.status}`;
      if (xhr.response?.error) message = xhr.response.error;
      settled = true;
      reject(new Error(message));
    };
    xhr.onerror = () => { cleanup(); if (!settled) reject(new Error('upload chunk request failed')); };
    xhr.onabort = () => { cleanup(); if (!settled) reject(new DOMException('upload aborted', 'AbortError')); };
    xhr.send(bytes);
  });
}

export async function uploadResumable(
  bucket: string,
  key: string,
  file: File,
  csrf: string | null | undefined,
  onProgress?: (sent: number, total: number) => void,
  options: ResumableUploadOptions = {}
) {
  let session: ResumableUploadSession;
  if (options.receptionId) {
    try {
      session = await resumableStatus(options.receptionId, csrf);
    } catch (cause) {
      // A persisted browser descriptor can outlive the in-memory server
      // reception. Re-selecting the same file starts a fresh reception rather
      // than retrying an ID that can never become active again.
      if (!(cause instanceof ApiError) || cause.status !== 404) throw cause;
      session = await beginResumableUpload(bucket, key, file.type || 'application/octet-stream', csrf, options.expiresInSeconds);
    }
  } else {
    session = await beginResumableUpload(bucket, key, file.type || 'application/octet-stream', csrf, options.expiresInSeconds);
  }
  options.onReception?.(session.id);
  let offset = session.received;
  const total = file.size;
  const maxFailures = 8;
  let emptyChunkSent = false;

  while (offset < total || (total === 0 && !emptyChunkSent)) {
    await options.waitUntilResumed?.();
    if (options.signal?.aborted) throw new DOMException('upload aborted', 'AbortError');
    const end = Math.min(total, offset + session.chunk_size);
    const chunk = file.slice(offset, end);
    const final = end >= total;
    let failureCount = 0;
    while (true) {
      try {
        const result = await sendResumableChunk(
          session.id,
          offset,
          chunk,
          final,
          csrf,
          options.signal,
          (sent) => onProgress?.(offset + sent, total)
        );
        offset = result.received;
        emptyChunkSent = true;
        onProgress?.(offset, total);
        break;
      } catch (cause) {
        if (options.signal?.aborted || (cause instanceof DOMException && cause.name === 'AbortError')) throw cause;
        failureCount += 1;
        session = await resumableStatus(session.id, csrf);
        offset = session.received;
        onProgress?.(offset, total);
        if (offset >= total) break;
        if (failureCount >= maxFailures) throw cause;
        await new Promise((resolve) => setTimeout(resolve, Math.min(8000, 700 * 2 ** (failureCount - 1))));
        await options.waitUntilResumed?.();
      }
    }
  }
  return completeResumableUpload(session.id, csrf);
}

export function getWizardState(csrf?: string | null) {
  return requestJson<WizardState>('/telegram/wizard/state', csrf);
}

export function wizardBegin(phone: string | undefined, flowId: string, csrf?: string | null) {
  return requestJson<WizardState>('/telegram/wizard/begin', csrf, {
    method: 'POST',
    body: phone === undefined ? { flow_id: flowId, replace: true } : { phone, flow_id: flowId, replace: true }
  });
}

export function wizardSubmitCode(code: string, flowId: string, csrf?: string | null) {
  return requestJson<WizardState>('/telegram/wizard/submit-code', csrf, {
    method: 'POST',
    body: { code, flow_id: flowId }
  });
}

export function wizardSubmitPassword(password: string, flowId: string, csrf?: string | null) {
  return requestJson<WizardState>('/telegram/wizard/submit-password', csrf, {
    method: 'POST',
    body: { password, flow_id: flowId }
  });
}

export function wizardCancel(flowId?: string, csrf?: string | null) {
  return requestJson<{ ok: boolean }>('/telegram/wizard/cancel', csrf, {
    method: 'POST',
    body: flowId ? { flow_id: flowId } : {}
  });
}

export function getSetup(){return requestJson<{setup_required:boolean}>('/setup');}
export function setupAccount(username:string,password:string){return requestJson<SessionState>('/setup',null,{method:'POST',body:{username,password}});}
export function listJobs(offset=0){return requestJson<{jobs:TransferJob[];next_offset:number|null}>(`/jobs?offset=${offset}&limit=50`);}
export function getJob(id:string){return requestJson<TransferJob>(`/jobs/${id}`);}
export function jobAction(id:string,action:'retry'|'cancel',csrf?:string|null){return requestJson(`/jobs/${id}/${action}`,csrf,{method:'POST'});}
