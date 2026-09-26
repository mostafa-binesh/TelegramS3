<script lang="ts">
  import { contentUrl } from '../lib/api';
  import { formatBytes, formatTimestamp } from '../lib/format';
  import LoadError from './LoadError.svelte';
  import ActionIcon from './ActionIcon.svelte';
  import type { BucketInfo, ObjectEntry, ObjectsState } from '../lib/types';

  export let buckets: BucketInfo[] = [];
  export let selectedBucket = '';
  export let currentPrefix = '';
  export let listing: ObjectsState | null = null;
  export let bucketsLoading = false;
  export let objectsLoading = false;
  export let bucketsError = '';
  export let objectsError = '';
  export let busy = false;
  export let selectedKeys: string[] = [];
  export let onCreateBucket: () => void = () => {};
  export let onRefresh: () => void = () => {};
  export let onUpload: () => void = () => {};
  export let onOpenBucket: (name: string) => void = () => {};
  export let onBack: () => void = () => {};
  export let onEnterFolder: (name: string) => void = () => {};
  export let onOpenFolder: () => void = () => {};
  export let onToggleKey: (key: string) => void = () => {};
  export let onToggleAll: () => void = () => {};
  export let onRemoveKey: (object: ObjectEntry | string) => void = () => {};
  export let onRemoveSelected: () => void = () => {};
  export let onRemoveBucket: (name: string) => void = () => {};
  export let onOpenMove: () => void = () => {};
  export let onShare: (object: ObjectEntry) => void = () => {};
  export let onOpenShareLinks: (object: ObjectEntry) => void = () => {};

  $: selectableObjects = listing?.objects.filter((object) => !object.uploading) ?? [];
  $: allVisibleSelected = selectedKeys.length > 0 && selectedKeys.length === selectableObjects.length;

  function uploadTitle(object: ObjectEntry) {
    const done = object.upload_parts_done ?? 0;
    const total = object.upload_parts_total ?? 0;
    const state = uploadKind(object) === 'attention'
      ? 'Upload needs attention; open Transfers to reconcile it'
      : uploadKind(object) === 'waiting'
        ? 'Telegram requested a short pause; retry is scheduled automatically'
      : uploadKind(object) === 'receiving'
        ? 'Receiving upload from S3'
        : uploadKind(object) === 'finalizing'
          ? 'Finalizing the object manifest'
          : uploadKind(object) === 'preparing'
            ? 'Preparing multipart upload'
            : 'Uploading to Telegram';
    return total > 0 ? `${state} — ${done} of ${total} parts completed` : `${state} — part total will appear when reception finishes`;
  }

  function uploadKind(object: ObjectEntry) {
    if (object.upload_state === 'recovery_required') return 'attention';
    if (object.upload_state === 'retry_wait') return 'waiting';
    if (object.upload_state === 'receiving') return 'receiving';
    const done = object.upload_parts_done ?? 0;
    const total = object.upload_parts_total ?? 0;
    if (object.upload_state === 'completing' || (total > 0 && done >= total)) return 'finalizing';
    if (object.upload_state === 'initiated' || total === 0) return 'preparing';
    return 'uploading';
  }

  function uploadLabel(object: ObjectEntry) {
    const kind = uploadKind(object);
    if (kind === 'attention') return 'Needs attention';
    if (kind === 'waiting') return 'Waiting for Telegram';
    if (kind === 'receiving') return 'Receiving from S3';
    if (kind === 'finalizing') return 'Finalizing backup';
    if (kind === 'preparing') return 'Preparing upload';
    return 'Uploading to Telegram';
  }

  function uploadDetail(object: ObjectEntry) {
    const kind = uploadKind(object);
    if (kind === 'attention') return 'Open Transfers to review';
    if (kind === 'waiting') return 'Retrying automatically after Telegram pacing';
    if (kind === 'receiving') return 'Parts appear as they arrive';
    if (kind === 'finalizing') return 'Publishing the final manifest';
    if (kind === 'preparing') return 'Waiting for the first part';
    return 'Encrypted parts are being secured';
  }

  function uploadPercent(object: ObjectEntry) {
    const done = object.upload_parts_done ?? 0;
    const total = object.upload_parts_total ?? 0;
    return total > 0 ? Math.min(100, Math.max(0, Math.round((done / total) * 100))) : 0;
  }
</script>

<section class="card surface">
  <div class="section-head"><div class="heading-row">{#if selectedBucket}<button class="icon-button back-button" type="button" title="Back to parent" aria-label="Back to parent folder" on:click={onBack}>←</button>{/if}<div><p class="card-label">Buckets and files</p><h2>{selectedBucket ? `Bucket / ${selectedBucket}${currentPrefix ? ` / ${currentPrefix.split('/').filter(Boolean).join(' / ')}` : ''}` : 'Your buckets'}</h2></div></div>
    <div class="toolbar-actions">
      {#if !selectedBucket}<button class="primary" type="button" on:click={onCreateBucket}>＋ Create bucket</button>{/if}
      <button class="ghost" type="button" on:click={onRefresh} disabled={busy || bucketsLoading || objectsLoading}>{#if bucketsLoading || objectsLoading}<span class="spinner" aria-hidden="true"></span>{/if}Refresh</button>
      {#if selectedBucket}<button class="ghost" type="button" on:click={onOpenFolder} disabled={busy}><span aria-hidden="true">＋</span> New folder</button><button class="primary" type="button" on:click={onUpload}><span aria-hidden="true">↑</span> Upload</button>{/if}
    </div>
  </div>
  {#if !selectedBucket}
    <p class="fine-print">Select a bucket to browse its files. Bucket names may contain Unicode characters.</p>
    {#if bucketsLoading}<div class="skeleton-stack" aria-label="Loading buckets"><div class="skeleton" style="height:52px"></div><div class="skeleton" style="height:52px"></div><div class="skeleton" style="height:52px"></div></div>
    {:else if bucketsError}<LoadError title="Could not load buckets" message={bucketsError} onRetry={onRefresh} />
    {:else if buckets.length === 0}<p class="empty-state"><span class="empty-mark" aria-hidden="true">+</span>No buckets yet. Create one above to start the file browser.</p>
    {:else}<ul class="checks">{#each buckets as bucket (bucket.name)}<li><div class="bucket-row"><button type="button" class="btn-link" on:click={() => onOpenBucket(bucket.name)}>{bucket.name}<small>created {formatTimestamp(bucket.created_at)}</small></button><ActionIcon name="trash" label={`Delete bucket ${bucket.name}`} tone="danger" on:click={() => onRemoveBucket(bucket.name)} disabled={busy}/></div></li>{/each}</ul>{/if}
  {:else}
    {#if listing}
      <div class="listing-frame" class:loading={objectsLoading} aria-busy={objectsLoading}>
        {#if objectsLoading}
          <div class="listing-status" role="status" aria-live="polite"><span class="spinner" aria-hidden="true"></span>Loading folder contents…</div>
        {:else if objectsError}
          <div class="listing-status error" role="alert"><span>{objectsError}</span><button class="ghost" type="button" on:click={onRefresh}>Retry</button></div>
        {/if}
        {#if listing.folders.length === 0 && listing.objects.length === 0}<p class="empty-state"><span class="empty-mark" aria-hidden="true">↑</span>This folder is empty. Drop files above to upload the first one.</p>
        {:else}<div class="table-scroll" class:listing-dimmed={objectsLoading}><table class="kv-table"><colgroup><col class="selection-column"/><col class="name-column"/><col class="size-column"/><col class="modified-column"/><col class="actions-column"/></colgroup><thead><tr><th><input class="select-all" type="checkbox" aria-label="Select all visible items" checked={allVisibleSelected} on:change={onToggleAll}/></th><th>Name</th><th>Size</th><th>Modified</th><th><span class="visually-hidden">Actions</span></th></tr></thead><tbody>
      {#each listing?.folders ?? [] as folder (folder)}<tr><td></td><td><button class="btn-link" on:click={() => onEnterFolder(folder)}>{folder}/</button></td><td class="muted">folder</td><td class="muted">—</td><td><div class="row-actions"><ActionIcon name="trash" label={`Delete folder ${folder}`} tone="danger" on:click={() => onRemoveKey(folder)} disabled={busy}/></div></td></tr>{/each}
      {#each listing?.objects ?? [] as obj (obj.key)}
        <tr class:uploading-row={obj.uploading} class:upload-attention={obj.uploading && uploadKind(obj) === 'attention'}>
          <td>
            {#if obj.uploading}
              <span class="uploading-icon" class:attention-icon={uploadKind(obj) === 'attention'} role="img" aria-label={uploadTitle(obj)} title={uploadTitle(obj)}>
                {#if uploadKind(obj) === 'attention'}
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 8v5m0 3.5v.01M10.3 3.8 2.7 17a2 2 0 0 0 1.73 3h15.14a2 2 0 0 0 1.73-3L13.7 3.8a2 2 0 0 0-3.4 0Z"/></svg>
                {:else}
                  <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 17.5H6.5a4 4 0 0 1-.36-7.98A6 6 0 0 1 17.6 8.1a4.5 4.5 0 0 1-.1 9.4H16M12 19V9m0 0-3 3m3-3 3 3"/></svg>
                {/if}
                <span class="status-beacon" aria-hidden="true"></span>
              </span>
            {:else}
              <input class="select-all" type="checkbox" checked={selectedKeys.includes(obj.key)} on:change={() => onToggleKey(obj.key)} aria-label={`Select ${obj.name}`}/>
            {/if}
          </td>
          <td>
            <span class="object-name">{obj.name}</span>
            {#if obj.uploading}
              <div class="upload-progress" class:attention-progress={uploadKind(obj) === 'attention'}>
                <div class="upload-progress-head">
                  <span class="upload-phase"><span class="phase-dot" aria-hidden="true"></span>{uploadLabel(obj)}</span>
                  {#if obj.upload_parts_total}<strong>{uploadPercent(obj)}%</strong>{/if}
                </div>
                <div
                  class="upload-track"
                  class:indeterminate={!obj.upload_parts_total}
                  role="progressbar"
                  aria-label={`Upload progress for ${obj.name}`}
                  aria-valuemin="0"
                  aria-valuemax="100"
                  aria-valuenow={obj.upload_parts_total ? uploadPercent(obj) : undefined}
                  aria-valuetext={obj.upload_parts_total ? `${obj.upload_parts_done ?? 0} of ${obj.upload_parts_total} parts completed` : 'Preparing multipart upload'}
                ><span class="upload-fill" style:width={`${uploadPercent(obj)}%`}></span></div>
                <div class="upload-meta">
                  <span>{#if obj.upload_parts_total}{obj.upload_parts_done ?? 0} of {obj.upload_parts_total} parts{:else}Discovering parts…{/if}</span>
                  <span>{uploadDetail(obj)}</span>
                </div>
              </div>
            {:else if obj.expires_at}
              <small class="expiry-note">expires {formatTimestamp(obj.expires_at)}</small>
            {/if}
          </td>
          <td>{#if obj.uploading}<span class="upload-size"><strong>Multipart</strong><small>S3 upload</small></span>{:else}{formatBytes(obj.size)}{/if}</td>
          <td>{#if obj.uploading}<span class="upload-state"><strong>{uploadLabel(obj)}</strong><small>{formatTimestamp(obj.last_modified)}</small></span>{:else}{formatTimestamp(obj.last_modified)}{/if}</td>
          <td><div class="row-actions">{#if obj.uploading}<span class="uploading-actions" class:needs-action={uploadKind(obj) === 'attention'} title={uploadTitle(obj)}><span aria-hidden="true"></span>{uploadKind(obj) === 'attention' ? 'Needs action' : 'Working'}</span>{:else}<ActionIcon name="download" label={`Download ${obj.name}`} href={contentUrl(selectedBucket, obj.key)}/><ActionIcon name="share" label={`Share ${obj.name}`} on:click={() => onShare(obj)} disabled={busy}/><ActionIcon name="links" badge={obj.shared_links} label={`Manage shared links for ${obj.name}`} on:click={() => onOpenShareLinks(obj)} disabled={busy}/><ActionIcon name="trash" label={`Delete ${obj.name}`} tone="danger" on:click={() => onRemoveKey(obj)} disabled={busy}/>{/if}</div></td>
        </tr>
      {/each}
    </tbody></table></div>{/if}
      </div>
    {:else if objectsLoading}<div class="skeleton-stack" aria-label="Loading files"><div class="skeleton" style="height:40px"></div><div class="skeleton" style="height:40px"></div><div class="skeleton" style="height:40px"></div></div>
    {:else if objectsError}<LoadError title="Could not load this folder" message={objectsError} onRetry={onRefresh} />
    {/if}
    {#if selectedKeys.length}<div class="selection-bar"><strong>{selectedKeys.length} selected</strong><button class="ghost bulk-action bulk-delete" type="button" on:click={onRemoveSelected}><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16m-10 4v6m4-6v6M9 7V4h6v3m-9 0 1 13h10l1-13"/></svg><span>Delete</span></button><button class="ghost bulk-action" type="button" on:click={onOpenMove}><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v18m0-18-3 3m3-3 3 3M12 21l-3-3m3 3 3-3M3 12h18m0 0-3-3m3 3-3 3M3 12l3-3m-3 3 3 3"/></svg><span>Move</span></button></div>{/if}
  {/if}
</section>
<style>
  .heading-row { display: flex; align-items: center; gap: .75rem; }
  .back-button { flex: 0 0 auto; }
  .listing-frame { position: relative; min-height: 72px; }
  .listing-status { display: flex; align-items: center; justify-content: flex-end; gap: 8px; min-height: 34px; margin: 0 0 8px; padding: 7px 10px; border: 1px solid #cfe4f1; border-radius: 10px; background: #f3faff; color: #2d6789; font-size: .78rem; font-weight: 750; }
  .listing-status.error { justify-content: space-between; gap: 12px; border-color: #f0c9c9; background: #fff7f7; color: var(--danger, #b00020); }
  .listing-status .ghost { flex: 0 0 auto; padding: 5px 10px; }
  .listing-frame .table-scroll { transition: opacity 180ms ease, filter 180ms ease; }
  .listing-frame .listing-dimmed { opacity: .48; filter: saturate(.7); pointer-events: none; }
  .bucket-row { display: flex; align-items: center; justify-content: space-between; gap: 1rem; }
  .expiry-note { display:block; color:var(--muted); font-size:.75rem; }
  .kv-table { table-layout: fixed; min-width: 940px; }
  .selection-column { width: 44px; }
  .size-column { width: 120px; }
  .modified-column { width: 172px; }
  .actions-column { width: 200px; }
  .kv-table th, .kv-table td { vertical-align: middle; }
  .kv-table th:nth-child(2), .kv-table td:nth-child(2) { overflow-wrap: anywhere; }
  .row-actions { display: flex; align-items: center; justify-content: flex-end; gap: 8px; min-height: 38px; white-space: nowrap; }
  .row-actions :global(.action-icon) { flex: 0 0 38px; }
  .selection-bar .bulk-action { display: inline-flex; align-items: center; gap: 6px; }
  .selection-bar .bulk-action svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
  .selection-bar .bulk-delete { color: var(--danger, #b00020); }
  .uploading-row { background: linear-gradient(90deg, rgba(237,247,255,.82), rgba(250,253,255,.45)); }
  .uploading-row td { padding-top: 16px; padding-bottom: 16px; border-color: #dcebf5; }
  .uploading-row.upload-attention { background: linear-gradient(90deg, rgba(255,247,231,.88), rgba(255,252,246,.5)); }
  .uploading-icon { position: relative; display: inline-grid; place-items: center; width: 36px; height: 36px; border: 1px solid #9acbe8; border-radius: 13px; background: linear-gradient(145deg,#f3fbff,#dff2ff); color: #2178ad; cursor: help; box-shadow: 0 8px 20px rgba(33,109,186,.12); }
  .uploading-icon svg { width: 21px; height: 21px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
  .status-beacon { position: absolute; right: -3px; bottom: -3px; width: 10px; height: 10px; border: 2px solid var(--surface); border-radius: 50%; background: #28a783; box-shadow: 0 0 0 0 rgba(40,167,131,.35); animation: beacon 1.8s ease-out infinite; }
  .uploading-icon.attention-icon { border-color: #edc27c; background: linear-gradient(145deg,#fffaf0,#ffedcb); color: #a66a0b; box-shadow: 0 8px 20px rgba(166,106,11,.1); }
  .attention-icon .status-beacon { background: #d88917; animation: none; }
  .object-name { display: block; overflow-wrap: anywhere; font-weight: 750; color: #203b57; }
  .upload-progress { display: grid; gap: 7px; max-width: 520px; margin-top: 10px; padding: 10px 12px; border: 1px solid #d3e8f5; border-radius: 13px; background: rgba(255,255,255,.78); box-shadow: 0 5px 15px rgba(35,90,130,.04); }
  .upload-progress-head, .upload-meta { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .upload-progress-head strong { color: #1c6f9f; font-size: .72rem; font-variant-numeric: tabular-nums; }
  .upload-phase { display: inline-flex; align-items: center; gap: 7px; color: #245f85; font-size: .72rem; font-weight: 850; letter-spacing: .01em; }
  .phase-dot { width: 7px; height: 7px; border-radius: 50%; background: #28a783; box-shadow: 0 0 0 4px rgba(40,167,131,.1); }
  .upload-track { position: relative; height: 7px; overflow: hidden; border-radius: 999px; background: #dcecf5; box-shadow: inset 0 1px 2px rgba(27,77,111,.08); }
  .upload-fill { display: block; height: 100%; min-width: 2px; border-radius: inherit; background: linear-gradient(90deg,#2b82c5,#34b09d); transition: width .35s ease; }
  .upload-fill::after { content: ''; display: block; width: 35%; height: 100%; background: linear-gradient(90deg,transparent,rgba(255,255,255,.7),transparent); animation: shimmer 1.9s ease-in-out infinite; }
  .upload-track.indeterminate .upload-fill { width: 38%!important; animation: indeterminate 1.7s ease-in-out infinite; }
  .upload-meta { color: #6b8194; font-size: .67rem; line-height: 1.3; }
  .upload-meta span:first-child { color: #3f6681; font-weight: 750; font-variant-numeric: tabular-nums; }
  .attention-progress { border-color: #efd8ad; background: rgba(255,253,248,.88); }
  .attention-progress .upload-phase, .attention-progress .upload-progress-head strong { color: #9a630f; }
  .attention-progress .phase-dot { background: #d88917; box-shadow: 0 0 0 4px rgba(216,137,23,.1); }
  .attention-progress .upload-track { background: #f2e4ca; }
  .attention-progress .upload-fill { background: linear-gradient(90deg,#d9972b,#e4b14f); }
  .upload-size, .upload-state { display: grid; gap: 3px; color: #536f84; }
  .upload-size strong, .upload-state strong { color: #345d78; font-size: .73rem; }
  .upload-size small, .upload-state small { color: #7d8f9e; font-size: .65rem; line-height: 1.3; }
  .uploading-actions { display: inline-flex; align-items: center; gap: 6px; padding: 6px 9px; border: 1px solid #cbe4f3; border-radius: 999px; background: #f1faff; color: #2178ad; font-size: .68rem; font-weight: 850; cursor: help; white-space: nowrap; }
  .uploading-actions > span { width: 6px; height: 6px; border-radius: 50%; background: #28a783; }
  .uploading-actions.needs-action { border-color: #ecd4a7; background: #fff8e9; color: #9a630f; }
  .uploading-actions.needs-action > span { background: #d88917; }
  @keyframes beacon { 70%,100% { box-shadow: 0 0 0 7px rgba(40,167,131,0); } }
  @keyframes shimmer { from { transform: translateX(-160%); } to { transform: translateX(380%); } }
  @keyframes indeterminate { 0% { transform: translateX(-110%); } 55%,100% { transform: translateX(270%); } }
  @media (prefers-reduced-motion: reduce) { .status-beacon, .upload-fill, .upload-fill::after { animation: none!important; transition: none; } .listing-frame .table-scroll { transition: none; } }
  .visually-hidden { position:absolute!important; width:1px!important; height:1px!important; padding:0!important; margin:-1px!important; overflow:hidden!important; clip:rect(0,0,0,0)!important; white-space:nowrap!important; border:0!important; }
</style>
