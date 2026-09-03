<script lang="ts">
  import { uploadObject } from '../lib/api';

  export let bucket: string;
  export let prefix = '';
  export let csrf: string | null | undefined;
  export let onUploaded: () => void = () => {};

  interface QueueItem {
    file: File;
    fullKey: string;
    progress: number; // 0..1
    busy: boolean;
    error?: string;
  }

  let items: QueueItem[] = [];
  let dragging = false;
  let uploadStarted = false;

  function buildKey(fileName: string) {
    const name = fileName.replace(/^\/+/, '');
    return `${prefix}${name}`;
  }

  function enqueue(files: FileList | File[] | null) {
    if (!files || files.length === 0) return;
    const incoming: QueueItem[] = Array.from(files).map((file) => ({
      file,
      fullKey: buildKey(file.name),
      progress: 0,
      busy: false
    }));
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

  async function startQueued() {
    if (uploadStarted) return;
    uploadStarted = true;
    try {
      await Promise.all(items.map((_, index) => doUpload(index)));
    } finally {
      uploadStarted = false;
    }
  }

  async function doUpload(index: number) {
    const item = items[index];
    if (!item) return;
    setItem(index, { busy: true, error: undefined });
    try {
      const total = item.file.size;
      await uploadObject(
        bucket,
        item.fullKey,
        item.file,
        csrf,
        (sent: number) => {
          setItem(index, { progress: total > 0 ? sent / total : 1 });
        }
      );
      setItem(index, { progress: 1, busy: false });
      onUploaded();
    } catch (cause) {
      setItem(index, {
        busy: false,
        error: cause instanceof Error ? cause.message : 'Unexpected upload failure'
      });
    }
  }

  function onDrop(event: DragEvent) {
    event.preventDefault();
    dragging = false;
    if (event.dataTransfer?.files && event.dataTransfer.files.length > 0) {
      enqueue(event.dataTransfer.files);
    }
  }

  function onDragOver(event: DragEvent) {
    event.preventDefault();
    dragging = true;
  }

  function onDragLeave() {
    dragging = false;
  }

  function removeItem(index: number) {
    items = items.filter((_, i) => i !== index);
  }
</script>

<div
  class:dropzone={dragging}
  class="upload-box"
  role="region"
  aria-label="Drop files to upload, or choose files below"
  on:dragover={onDragOver}
  on:dragleave={onDragLeave}
  on:drop={onDrop}
>
  <div class="row-inline">
    <label class="file-picker">
      <span>Upload into {prefix ? `“${prefix}”` : 'bucket root'}</span>
      <input type="file" multiple accept="*/*" on:change={onInputChange} />
    </label>
    {#if items.length > 0}
      <button
        class="primary"
        type="button"
        on:click={startQueued}
        disabled={uploadStarted || items.every((item) => item.progress === 1 && !item.error)}
      >
        {uploadStarted ? 'Uploading…' : `Upload ${items.length}`}
      </button>
    {/if}
  </div>
  {#if dragging}
    <div class="drop-hint">Drop files to upload here</div>
  {/if}

  {#if items.length > 0}
    <ul class="upload-queue">
      {#each items as item, i (item.fullKey + item.file.lastModified)}
        <li>
          <div class="queue-meta">
            <span class="queue-name">{item.file.name}</span>
            <span class="queue-sub">
              {item.error ? 'failed' : item.busy ? 'uploading' : item.progress === 1 ? 'done' : 'queued'}
            </span>
            <button class="queue-remove" type="button" on:click={() => removeItem(i)} disabled={item.busy}>
              ✕
            </button>
          </div>
          <div class="bar-track" aria-hidden="true">
            <div class="bar-fill" style:width={Math.round(item.progress * 100) + '%'}></div>
          </div>
          {#if item.error}
            <p class="fine-print error-hint">{item.error}</p>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .upload-box {
    border: 1px dashed var(--border);
    border-radius: 16px;
    padding: 12px;
    margin: 8px 0 4px;
    background: rgba(255, 255, 255, 0.5);
    transition: background 120ms ease;
  }
  .dropzone {
    background: rgba(13, 122, 109, 0.1);
    border-color: rgba(13, 122, 109, 0.45);
  }
  .drop-hint {
    margin-top: 10px;
    font-weight: 700;
    color: var(--accent, #0d7a6d);
  }
  .row-inline {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .file-picker {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-weight: 700;
    cursor: pointer;
  }
  .file-picker input {
    max-width: 220px;
    padding: 6px;
  }
  .upload-queue {
    list-style: none;
    margin: 10px 0 0;
    padding: 0;
    display: grid;
    gap: 8px;
  }
  .upload-queue li {
    display: grid;
    gap: 4px;
    padding: 8px 10px;
    border-radius: 12px;
    background: rgba(255, 255, 255, 0.8);
    border: 1px solid var(--border);
  }
  .queue-meta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .queue-name {
    font-weight: 700;
    word-break: break-all;
  }
  .queue-sub {
    font-size: 0.82rem;
    color: var(--muted);
  }
  .queue-remove {
    flex: 0 0 auto;
    padding: 0.2rem 0.55rem;
    background: transparent;
    color: var(--muted);
    border: 1px solid var(--border);
  }
  .bar-track {
    height: 6px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text, #172033) 14%, transparent);
    overflow: hidden;
  }
  .bar-fill {
    height: 100%;
    border-radius: inherit;
    background: linear-gradient(90deg, var(--accent), #12648d);
    transition: width 120ms linear;
  }
  .error-hint {
    margin: 0;
    color: var(--danger, #b00020);
  }
</style>
