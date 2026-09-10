<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { abortResumableUpload, getJob, uploadResumable } from '../lib/api';

  export let bucket: string;
  export let prefix = '';
  export let csrf: string | null | undefined;
  export let onUploaded: () => void = () => {};
  export let onUploadActivity: (active: boolean) => void = () => {};

  interface SavedUpload {
    id: string;
    bucket: string;
    key: string;
    name: string;
    size: number;
    lastModified: number;
  }

  interface QueueItem {
    file: File;
    fullKey: string;
    progress: number;
    busy: boolean;
    paused: boolean;
    cancelled: boolean;
    receptionId?: string;
    jobId?: string;
    error?: string;
    state?: string;
    chunksDone?: number;
    chunksTotal?: number;
  }

  const STORAGE_KEY = 'telegram-s3-admin-resumable-uploads';
  let items: QueueItem[] = [];
  let saved: SavedUpload[] = [];
  let dragging = false;
  let uploadStarted = false;
  let componentActive = true;
  let serverUploadCount = 0;
  let expiresInSeconds = '';
  let expiryError = '';
  let selectedExpiry: number | null = null;
  const controllers = new Map<number, AbortController>();

  function setServerUploadActivity(active: boolean) {
    serverUploadCount = Math.max(0, serverUploadCount + (active ? 1 : -1));
    onUploadActivity(serverUploadCount > 0);
  }

  function buildKey(fileName: string) {
    const name = fileName.replace(/^\/+/, '');
    return `${prefix}${name}`;
  }

  function readSaved() {
    try { saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '[]') as SavedUpload[]; }
    catch { saved = []; }
  }

  function writeSaved() {
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(saved)); } catch { /* storage is optional */ }
  }

  function remember(item: QueueItem) {
    if (!item.receptionId) return;
    saved = saved.filter((entry) => !(entry.bucket === bucket && entry.key === item.fullKey));
    saved = [...saved, { id: item.receptionId, bucket, key: item.fullKey, name: item.file.name, size: item.file.size, lastModified: item.file.lastModified }];
    writeSaved();
  }

  function forget(item: QueueItem) {
    saved = saved.filter((entry) => entry.id !== item.receptionId);
    writeSaved();
  }

  function matchingSaved(file: File, fullKey: string) {
    return saved.find((entry) => entry.bucket === bucket && entry.key === fullKey && entry.name === file.name && entry.size === file.size && entry.lastModified === file.lastModified);
  }

  function enqueue(files: FileList | File[] | null) {
    if (!files || files.length === 0) return;
    const incoming: QueueItem[] = Array.from(files).map((file) => {
      const fullKey = buildKey(file.name);
      const previous = matchingSaved(file, fullKey);
      return { file, fullKey, progress: 0, busy: false, paused: false, cancelled: false, receptionId: previous?.id, state: previous ? 'resume available' : undefined };
    });
    items = items.concat(incoming);
  }

  function onInputChange(event: Event) {
    const input = event.target as HTMLInputElement;
    if (input.files && input.files.length > 0) enqueue(input.files);
    input.value = '';
  }

  function setItem(index: number, patch: Partial<QueueItem>) {
    items = items.map((item, i) => (i === index ? { ...item, ...patch } : item));
  }

  async function waitUntilResumed(index: number) {
    while (items[index]?.paused && !items[index]?.cancelled) await new Promise((resolve) => setTimeout(resolve, 150));
  }

  async function startQueued() {
    if (uploadStarted) return;
    expiryError = '';
    if (expiresInSeconds.trim() && (!/^\d+$/.test(expiresInSeconds.trim()) || Number(expiresInSeconds) < 1)) {
      expiryError = 'Expiry must be a positive number of seconds.';
      return;
    }
    selectedExpiry = expiresInSeconds.trim() ? Number(expiresInSeconds) : null;
    uploadStarted = true;
    try {
      for (let index = 0; index < items.length; index += 1) await doUpload(index);
    } finally { uploadStarted = false; }
  }

  async function doUpload(index: number) {
    const item = items[index];
    if (!item || item.jobId || item.cancelled) return;
    const controller = new AbortController();
    controllers.set(index, controller);
    setItem(index, { busy: true, error: undefined, paused: false });
    try {
      let accepted;
      setServerUploadActivity(true);
      try {
        accepted = await uploadResumable(bucket, item.fullKey, item.file, csrf, (sent, total) => {
          setItem(index, { progress: total ? sent / total : 1 });
        }, {
          signal: controller.signal,
          receptionId: item.receptionId,
          onReception: (id) => { setItem(index, { receptionId: id, state: 'receiving' }); remember({ ...items[index], receptionId: id }); },
          waitUntilResumed: () => waitUntilResumed(index),
          expiresInSeconds: selectedExpiry
        });
      } finally {
        setServerUploadActivity(false);
      }
      setItem(index, { progress: 0.02, busy: false, jobId: accepted.job_id, state: 'queued', receptionId: undefined });
      forget({ ...item, receptionId: item.receptionId });
      for (let poll = 0; poll < 180; poll += 1) {
        const job = await getJob(accepted.job_id);
        const totalChunks = Math.max(job.chunks_total, 1);
        setItem(index, { state: job.state, chunksDone: job.chunks_done, chunksTotal: job.chunks_total, progress: ['completed', 'cleaned'].includes(job.state) ? 1 : Math.min(0.99, job.chunks_done / totalChunks) });
        if (['completed', 'cleaned'].includes(job.state)) {
          if (componentActive) onUploaded();
          return;
        }
        if (['recovery_required', 'reception_failed', 'cancelled'].includes(job.state)) throw new Error(job.error || `Transfer ${job.state.replaceAll('_', ' ')}`);
        await new Promise((resolve) => setTimeout(resolve, 1000));
      }
      throw new Error('Transfer is taking longer than expected; follow it in Transfers.');
    } catch (cause) {
      if (!items[index]?.cancelled) setItem(index, { busy: false, error: cause instanceof Error ? cause.message : 'Unexpected upload failure', state: 'resume available' });
    } finally { controllers.delete(index); }
  }

  function pauseItem(index: number) { setItem(index, { paused: true, state: 'paused' }); }
  function resumeItem(index: number) { setItem(index, { paused: false, error: undefined, state: 'receiving' }); if (!uploadStarted) void doUpload(index); }
  function displayState(state?: string) { return state === 'uploading' ? 'uploading to telegram' : state?.replaceAll('_', ' '); }

  async function cancelItem(index: number) {
    const item = items[index];
    if (!item) return;
    setItem(index, { cancelled: true, paused: false, busy: false, state: 'cancelled', error: undefined });
    controllers.get(index)?.abort();
    if (item.receptionId) {
      const receptionId = item.receptionId;
      void abortResumableUpload(receptionId, csrf).catch(() => { /* the session may already have expired */ });
      forget(item);
    }
  }

  export async function cancelActiveUploads() {
    await Promise.all(items.map((item, index) => item.busy ? cancelItem(index) : Promise.resolve()));
  }

  function onDrop(event: DragEvent) {
    event.preventDefault(); dragging = false;
    if (event.dataTransfer?.files && event.dataTransfer.files.length > 0) enqueue(event.dataTransfer.files);
  }
  function onDragOver(event: DragEvent) { event.preventDefault(); dragging = true; }
  function onDragLeave() { dragging = false; }
  function removeItem(index: number) { if (!items[index]?.busy) items = items.filter((_, i) => i !== index); }

  onMount(readSaved);
  onDestroy(() => {
    componentActive = false;
    controllers.forEach((controller) => controller.abort());
  });
</script>

<div class:dropzone={dragging} class="upload-box" role="region" aria-label="Drop files to upload, or choose files below" on:dragover={onDragOver} on:dragleave={onDragLeave} on:drop={onDrop}>
  <div class="upload-prompt"><span class="upload-glyph" aria-hidden="true">↑</span><div><strong>Drop files here</strong><span>Upload into {prefix ? `“${prefix}”` : 'bucket root'}</span></div><label class="choose-files"><span>Choose files</span><input class="visually-hidden" type="file" multiple accept="*/*" on:change={onInputChange} /></label></div>
  <div class="expiry-control"><label><span>Object expiry (optional)</span><input bind:value={expiresInSeconds} type="number" min="1" step="1" placeholder="Never" aria-describedby="expiry-help" /></label><span id="expiry-help" class="fine-print">Hide this object after this many seconds.</span></div>
  {#if expiryError}<p class="fine-print error-hint" role="alert">{expiryError}</p>{/if}
  {#if dragging}<div class="drop-hint">Release to add files</div>{/if}
  {#if items.length > 0}<div class="row-inline"><button class="primary" type="button" on:click={startQueued} disabled={uploadStarted || items.every((item) => item.jobId || item.cancelled)}>Upload {items.length}</button></div>{/if}
  {#if items.length > 0}<ul class="upload-queue">{#each items as item, i (item.fullKey + item.file.lastModified)}<li>
    <div class="queue-meta"><div><span class="queue-name">{item.file.name}</span><span class="queue-sub">{item.error ? displayState(item.state) ?? 'failed' : item.jobId ? `${displayState(item.state) ?? 'queued'}${item.chunksTotal ? ` · ${item.chunksDone ?? 0}/${item.chunksTotal} chunks` : ''}` : item.busy ? (item.paused ? 'Paused' : 'Receiving') : displayState(item.state) ?? 'Ready to send'}</span></div><div class="queue-actions">{#if item.busy && !item.paused}<button class="ghost" type="button" on:click={() => pauseItem(i)}>Pause</button>{:else if item.paused || item.error}<button class="ghost" type="button" on:click={() => resumeItem(i)}>Resume</button>{/if}{#if item.receptionId && !item.jobId && !item.cancelled}<button class="ghost" type="button" on:click={() => void cancelItem(i)}>Cancel</button>{:else}<button class="queue-remove" type="button" title="Remove from upload queue" aria-label={`Remove ${item.file.name} from upload queue`} on:click={() => removeItem(i)} disabled={item.busy}>×</button>{/if}</div></div>
    <div class="bar-track" aria-hidden="true"><div class="bar-fill" style:width={Math.round(item.progress * 100) + '%'}></div></div>{#if item.error}<p class="fine-print error-hint">{item.error}</p>{/if}
  </li>{/each}</ul>{/if}
</div>

<style>
  .upload-box{border:1px dashed var(--border);border-radius:var(--radius-lg);padding:16px;margin:8px 0 4px;background:rgba(255,255,255,.5);transition:background 120ms ease,border-color 120ms ease}.dropzone{background:var(--accent-soft);border-color:var(--accent-ring)}.upload-prompt{display:flex;align-items:center;gap:12px}.upload-prompt>div{display:grid;gap:4px;margin-right:auto}.upload-prompt>div span{font-size:.82rem;color:var(--muted)}.upload-glyph{display:grid;place-items:center;width:38px;height:38px;border-radius:50%;background:var(--accent-soft);color:var(--accent);font-size:1.5rem}.choose-files{display:inline-flex;align-items:center;border:1px solid var(--border);border-radius:var(--radius-sm);padding:.65rem .85rem;font-weight:700;cursor:pointer;background:var(--surface)}.expiry-control{display:grid;gap:4px;margin-top:14px}.expiry-control label{display:grid;grid-template-columns:minmax(150px,1fr) minmax(120px,180px);gap:10px;align-items:center}.expiry-control input{width:100%}.visually-hidden{position:absolute!important;width:1px!important;height:1px!important;padding:0!important;margin:-1px!important;overflow:hidden!important;clip:rect(0,0,0,0)!important;white-space:nowrap!important;border:0!important}.drop-hint{margin-top:10px;font-weight:700;color:var(--accent)}.row-inline{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin:12px 0 0}.upload-queue{list-style:none;margin:12px 0 0;padding:0;display:grid;gap:8px}.upload-queue li{display:grid;gap:6px;padding:9px 10px;border-radius:var(--radius-md);background:rgba(255,255,255,.8);border:1px solid var(--border)}.queue-meta{display:flex;align-items:center;justify-content:space-between;gap:8px}.queue-name{display:block;font-weight:700;word-break:break-all}.queue-sub{display:block;font-size:.82rem;color:var(--muted)}.queue-actions{display:flex;gap:4px;align-items:center}.queue-actions button{padding:.3rem .55rem;font-size:.78rem}.queue-remove{flex:0 0 auto;background:transparent;color:var(--muted);border:1px solid var(--border)}.bar-track{height:6px;border-radius:999px;background:color-mix(in srgb,var(--text) 14%,transparent);overflow:hidden}.bar-fill{height:100%;border-radius:inherit;background:linear-gradient(90deg,var(--accent),#12648d);transition:width 120ms linear}.error-hint{margin:0;color:var(--danger,#b00020)}@media(max-width:520px){.upload-prompt{align-items:flex-start;flex-wrap:wrap}.choose-files{margin-left:50px}.expiry-control label{grid-template-columns:1fr}}
</style>
