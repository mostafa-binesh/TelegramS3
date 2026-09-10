<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { abortResumableUpload, getJob, uploadResumable } from '../lib/api';
  import ActionIcon from './ActionIcon.svelte';

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
  const expiryPresets = [
    { label: 'Never', seconds: null },
    { label: '15 min', seconds: 900 },
    { label: '1 hour', seconds: 3600 },
    { label: '1 day', seconds: 86400 }
  ];

  $: pendingCount = items.filter((item) => !item.jobId && !item.cancelled).length;
  $: expiryPreset = expiryPresets.find((preset) => String(preset.seconds ?? '') === String(expiresInSeconds ?? '').trim());

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

  function chooseExpiry(seconds: number | null) {
    expiresInSeconds = seconds === null ? '' : String(seconds);
    expiryError = '';
  }

  function setItem(index: number, patch: Partial<QueueItem>) {
    items = items.map((item, i) => (i === index ? { ...item, ...patch } : item));
  }

  async function waitUntilResumed(index: number) {
    while (items[index]?.paused && !items[index]?.cancelled) await new Promise((resolve) => setTimeout(resolve, 150));
  }

  async function startQueued() {
    if (uploadStarted) return;
    const trimmedExpiry = String(expiresInSeconds ?? '').trim();
    expiryError = '';
    if (trimmedExpiry && (!/^\d+$/.test(trimmedExpiry) || Number(trimmedExpiry) < 1)) {
      expiryError = 'Expiry must be a positive number of seconds.';
      return;
    }
    selectedExpiry = trimmedExpiry ? Number(trimmedExpiry) : null;
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
  function formatFileSize(bytes: number) {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1073741824) return `${(bytes / 1048576).toFixed(1)} MB`;
    return `${(bytes / 1073741824).toFixed(1)} GB`;
  }
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

<div class:dropzone={dragging} class="upload-box" role="region" aria-label="Drop files to upload, or choose files below" aria-busy={uploadStarted} on:dragover={onDragOver} on:dragleave={onDragLeave} on:drop={onDrop}>
  <label class="drop-surface">
    <input class="visually-hidden" type="file" multiple accept="*/*" on:change={onInputChange} aria-label="Choose files to upload" />
    <span class="upload-glyph" aria-hidden="true">↑</span>
    <span class="upload-copy"><span class="upload-kicker">Add to {prefix ? prefix : 'bucket root'}</span><strong>{dragging ? 'Release to add files' : 'Drop files anywhere in this panel'}</strong><span>or click to browse from your device</span></span>
    <span class="choose-files">Browse files <span aria-hidden="true">↗</span></span>
  </label>
  <section class="expiry-control" aria-labelledby="expiry-title">
    <div class="expiry-heading"><span class="expiry-icon" aria-hidden="true">◷</span><div><strong id="expiry-title">Object expiry <span class="optional-tag">Optional</span></strong><span id="expiry-help">Automatically hide these files after a number of seconds.</span></div></div>
    <div class="expiry-options" aria-label="Expiry presets">
      {#each expiryPresets as preset}
        <button type="button" class:active={expiryPreset?.label === preset.label} class="expiry-preset" aria-pressed={expiryPreset?.label === preset.label} on:click={() => chooseExpiry(preset.seconds)} disabled={uploadStarted}>{preset.label}</button>
      {/each}
    </div>
    <label class="expiry-field"><span>Custom duration</span><div class="expiry-input-wrap"><input bind:value={expiresInSeconds} type="number" min="1" step="1" placeholder="No expiry" aria-label="Object expiry in seconds" aria-describedby="expiry-help" disabled={uploadStarted} /><span>seconds</span></div></label>
  </section>
  {#if expiryError}<p class="fine-print error-hint" role="alert">{expiryError}</p>{/if}
  {#if items.length > 0}<div class="queue-header"><div><div class="queue-title-row"><strong>{uploadStarted ? 'Uploading files' : `${pendingCount} ${pendingCount === 1 ? 'file' : 'files'} ready`}</strong>{#if uploadStarted}<span class="upload-status"><span class="status-dot"></span>In progress</span>{/if}</div><span>{uploadStarted ? 'You can keep this window open while transfers finish.' : 'Review the queue, then start the upload.'}</span></div><button class="primary" type="button" on:click={startQueued} disabled={uploadStarted || pendingCount === 0}>{uploadStarted ? 'Uploading…' : `Upload ${pendingCount}`}</button></div>{/if}
  {#if items.length > 0}<ul class="upload-queue">{#each items as item, i (item.fullKey + item.file.lastModified)}<li>
    <div class="queue-meta"><div class="file-identity"><span class="file-type" aria-hidden="true">↥</span><span><span class="queue-name">{item.file.name}</span><span class="queue-sub">{formatFileSize(item.file.size)} · {item.error ? displayState(item.state) ?? 'failed' : item.jobId ? `${displayState(item.state) ?? 'queued'}${item.chunksTotal ? ` · ${item.chunksDone ?? 0}/${item.chunksTotal} chunks` : ''}` : item.busy ? (item.paused ? 'Paused' : 'Receiving') : displayState(item.state) ?? 'Ready to send'}</span></span></div><div class="queue-actions">{#if item.busy && !item.paused}<button class="ghost compact" type="button" on:click={() => pauseItem(i)}>Pause</button>{:else if item.paused || item.error}<button class="ghost compact" type="button" on:click={() => resumeItem(i)}>Resume</button>{/if}{#if item.receptionId && !item.jobId && !item.cancelled}<button class="ghost compact" type="button" on:click={() => void cancelItem(i)}>Cancel</button>{:else}<ActionIcon name="cancel" label={`Remove ${item.file.name} from upload queue`} on:click={() => removeItem(i)} disabled={item.busy}/>{/if}</div></div>
    <div class="progress-row"><div class="bar-track" role="progressbar" aria-label={`Upload progress for ${item.file.name}`} aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(item.progress * 100)}><div class="bar-fill" style:width={Math.round(item.progress * 100) + '%'}></div></div><span>{Math.round(item.progress * 100)}%</span></div>{#if item.error}<p class="fine-print error-hint">{item.error}</p>{/if}
  </li>{/each}</ul>{/if}
</div>

<style>
  .upload-box{display:grid;gap:18px;margin:8px 0 4px;padding:18px;border:1px solid #d7e5f0;border-radius:22px;background:linear-gradient(145deg,#fbfdff,#f1f7fb);transition:background 160ms ease,border-color 160ms ease,box-shadow 160ms ease}.dropzone{border-color:#70b6df;background:linear-gradient(145deg,#eef9ff,#eaf5ff);box-shadow:0 0 0 4px rgba(33,109,186,.08)}.drop-surface{display:flex;align-items:center;gap:14px;min-height:118px;padding:20px;border:1.5px dashed #a9cce3;border-radius:18px;background:rgba(255,255,255,.76);cursor:pointer;transition:background 160ms ease,border-color 160ms ease,transform 160ms ease}.drop-surface:hover{border-color:#5ba4d2;background:#fff;transform:translateY(-1px)}.dropzone .drop-surface{border-color:#4097cc;background:rgba(255,255,255,.88)}.upload-glyph{display:grid;place-items:center;flex:0 0 auto;width:52px;height:52px;border-radius:17px;background:linear-gradient(135deg,#dceeff,#e8f6ff);color:#216dba;font-size:1.8rem;font-weight:800}.upload-copy{display:grid;gap:4px;margin-right:auto}.upload-kicker{font-size:.68rem;font-weight:800;letter-spacing:.12em;text-transform:uppercase;color:#5680a0}.upload-copy strong{font-size:1.04rem;letter-spacing:-.02em}.upload-copy>span:last-child{font-size:.82rem;color:var(--muted)}.choose-files{display:inline-flex;align-items:center;gap:7px;flex:0 0 auto;border:1px solid #a8d0e9;border-radius:12px;padding:.68rem .9rem;font-weight:800;background:#fff;color:#216dba}.expiry-control{display:grid;gap:13px;padding:16px;border:1px solid #dbe7ef;border-radius:17px;background:rgba(255,255,255,.72)}.expiry-heading{display:flex;align-items:center;gap:10px}.expiry-heading>div{display:grid;gap:3px}.expiry-heading strong{font-size:.9rem}.expiry-heading strong .optional-tag{display:inline-block;margin-left:5px;padding:3px 6px;border-radius:999px;background:#f1f5f8;color:#718398;font-size:.62rem;letter-spacing:.04em;text-transform:uppercase;vertical-align:middle}.expiry-heading>div>span{font-size:.76rem;color:var(--muted);line-height:1.35}.expiry-icon{display:grid;place-items:center;flex:0 0 auto;width:36px;height:36px;border-radius:12px;background:#fff3d6;color:#bb7b16;font-size:1.1rem}.expiry-options{display:flex;flex-wrap:wrap;gap:7px}.expiry-preset{min-height:36px;padding:.42rem .75rem;border:1px solid #d5e0e8;border-radius:10px;background:#fff;color:#53677c;font-size:.78rem;font-weight:800}.expiry-preset:hover:not(:disabled),.expiry-preset.active{border-color:#8fc1df;background:#edf8ff;color:#216dba}.expiry-preset.active{box-shadow:0 0 0 2px rgba(33,109,186,.08)}.expiry-field{display:flex;align-items:center;justify-content:space-between;gap:12px;color:#596b7e;font-size:.77rem;font-weight:800}.expiry-field>span{color:var(--text)}.expiry-input-wrap{display:flex;align-items:center;gap:8px;min-width:190px;color:var(--muted);font-size:.75rem}.expiry-input-wrap input{min-height:40px}.queue-header{display:flex;align-items:center;justify-content:space-between;gap:12px;padding-top:2px}.queue-header>div{display:grid;gap:4px}.queue-header>div>span{font-size:.78rem;color:var(--muted)}.queue-title-row{display:flex;align-items:center;gap:9px}.upload-status{display:inline-flex!important;align-items:center;gap:5px;padding:4px 7px;border-radius:999px;background:#e9f7f3;color:#197a61;font-size:.67rem!important;font-weight:800}.status-dot{width:6px;height:6px;border-radius:50%;background:currentColor}.upload-queue{list-style:none;margin:0;padding:0;display:grid;gap:9px}.upload-queue li{display:grid;gap:10px;padding:13px 14px;border-radius:15px;background:#fff;border:1px solid #dce7ee}.queue-meta{display:flex;align-items:center;justify-content:space-between;gap:10px}.file-identity{display:flex;align-items:center;gap:10px;min-width:0}.file-type{display:grid;place-items:center;flex:0 0 auto;width:32px;height:32px;border-radius:10px;background:#eef6fb;color:#317ba8;font-weight:900}.queue-name{display:block;font-weight:800;word-break:break-all}.queue-sub{display:block;font-size:.75rem;color:var(--muted);margin-top:3px}.queue-actions{display:flex;gap:5px;align-items:center}.queue-actions .compact{min-height:34px;padding:.4rem .65rem;font-size:.75rem}.progress-row{display:flex;align-items:center;gap:9px}.progress-row>span{min-width:35px;color:#60758a;font-size:.72rem;font-variant-numeric:tabular-nums;text-align:right}.bar-track{height:7px;flex:1;border-radius:999px;background:#e8eef3;overflow:hidden}.bar-fill{height:100%;border-radius:inherit;background:linear-gradient(90deg,#2e8fd1,#32b8a7);transition:width 120ms linear}.error-hint{margin:0;color:var(--danger,#b00020)}.visually-hidden{position:absolute!important;width:1px!important;height:1px!important;padding:0!important;margin:-1px!important;overflow:hidden!important;clip:rect(0,0,0,0)!important;white-space:nowrap!important;border:0!important}@media(max-width:560px){.drop-surface{align-items:flex-start;flex-wrap:wrap;padding:16px}.choose-files{margin-left:66px}.expiry-field{align-items:stretch;flex-direction:column;gap:7px}.expiry-input-wrap{width:100%;min-width:0}.queue-header{align-items:flex-start;flex-direction:column}.queue-header button{width:100%}.queue-meta{align-items:flex-start;flex-direction:column}.queue-actions{align-self:flex-end}}
</style>
