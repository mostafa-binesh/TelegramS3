<script lang="ts">
  import { contentUrl } from '../lib/api';
  import { formatBytes, formatTimestamp } from '../lib/format';
  import LoadError from './LoadError.svelte';
  import ActionIcon from './ActionIcon.svelte';
  import type { BucketInfo, ObjectEntry, ObjectsState, SearchResult } from '../lib/types';

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
  export let bucketSearch = '';
  export let bucketPage = 1;
  export let bucketTotal = 0;
  export let globalSearchResults: SearchResult[] = [];
  export let globalSearchPage = 1;
  export let globalSearchTotal = 0;
  export let globalSearchLoading = false;
  export let globalSearchError = '';
  export let objectSearch = '';
  export let objectPage = 1;
  export let objectTotal = 0;
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
  export let onRechunkSelected: () => void = () => {};
  export let onOpenReplicas: (bucket: string, key?: string) => void = () => {};
  export let onOpenBulkReplication: () => void = () => {};
  export let onShare: (object: ObjectEntry) => void = () => {};
  export let onOpenShareLinks: (object: ObjectEntry) => void = () => {};
  export let onBucketSearch: (value: string) => void = () => {};
  export let onBucketPage: (page: number) => void = () => {};
  export let onGlobalSearchPage: (page: number) => void = () => {};
  export let onOpenSearchResult: (result: SearchResult) => void = () => {};
  export let onObjectSearch: (value: string) => void = () => {};
  export let onObjectPage: (page: number) => void = () => {};
  export let onGoToFolder: (location: string) => void = () => {};

  type PageItem = number | 'ellipsis';

  $: selectableObjects = listing?.objects.filter((object) => !object.uploading) ?? [];
  $: allVisibleSelected = selectedKeys.length > 0 && selectedKeys.length === selectableObjects.length;
  $: bucketPageCount = Math.max(1, Math.ceil(bucketTotal / 25));
  $: globalSearchPageCount = Math.max(1, Math.ceil(globalSearchTotal / 25));
  $: objectPageCount = Math.max(1, Math.ceil(objectTotal / 25));
  $: bucketPageItems = pageItems(bucketPage, bucketPageCount);
  $: globalSearchPageItems = pageItems(globalSearchPage, globalSearchPageCount);
  $: objectPageItems = pageItems(objectPage, objectPageCount);

  function pageItems(current: number, total: number): PageItem[] {
    if (total <= 7) return Array.from({ length: total }, (_, index) => index + 1);

    const visible = new Set([1, total, current, current - 1, current + 1]);
    const pages: PageItem[] = [];
    for (let page = 1; page <= total; page += 1) {
      if (!visible.has(page)) {
        if (pages.at(-1) !== 'ellipsis') pages.push('ellipsis');
        continue;
      }
      pages.push(page);
    }
    return pages;
  }

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
    <div class="browser-toolbar">
      <label class="search-field">
        <span class="visually-hidden">Search buckets and files</span>
        <input value={bucketSearch} type="search" placeholder="Search buckets and files…" aria-label="Search buckets and files" on:input={(event) => onBucketSearch((event.currentTarget as HTMLInputElement).value)} />
      </label>
      {#if bucketSearch}<button class="btn-link clear-search" type="button" on:click={() => onBucketSearch('')}>Clear search</button>{/if}
    </div>
    <p class="fine-print">Select a bucket to browse its files. Search also checks object names recursively across every bucket.</p>
    {#if bucketsLoading}<div class="skeleton-stack" aria-label="Loading buckets"><div class="skeleton" style="height:52px"></div><div class="skeleton" style="height:52px"></div><div class="skeleton" style="height:52px"></div></div>
    {:else if bucketsError}<LoadError title="Could not load buckets" message={bucketsError} onRetry={onRefresh} />
    {:else if buckets.length === 0}<p class="empty-state"><span class="empty-mark" aria-hidden="true">{bucketSearch ? '⌕' : '+'}</span>{bucketSearch ? 'No buckets match this search.' : 'No buckets yet. Create one above to start the file browser.'}</p>
    {:else}<ul class="checks">{#each buckets as bucket (bucket.name)}<li><div class="bucket-row"><button type="button" class="btn-link" on:click={() => onOpenBucket(bucket.name)}>{bucket.name}<small>created {formatTimestamp(bucket.created_at)}</small></button><div class="bucket-meta">{#if (bucket.replica_accounts ?? 0) + (bucket.access_accounts ?? 0)}<ActionIcon name="accounts" tone="success" badge={(bucket.replica_accounts ?? 0) + (bucket.access_accounts ?? 0)} label={`Show account copies and access for ${bucket.name}`} on:click={() => onOpenReplicas(bucket.name)}/>{/if}<ActionIcon name="trash" label={`Delete bucket ${bucket.name}`} tone="danger" on:click={() => onRemoveBucket(bucket.name)} disabled={busy}/></div></div></li>{/each}</ul>{/if}
    {#if bucketTotal > 25}<nav class="pagination" aria-label="Bucket pages"><button class="pagination-arrow" type="button" aria-label="Previous bucket page" disabled={bucketPage <= 1 || bucketsLoading} on:click={() => onBucketPage(bucketPage - 1)}>←</button><div class="page-numbers">{#each bucketPageItems as item}{#if item === 'ellipsis'}<span class="pagination-ellipsis" aria-hidden="true">…</span>{:else}<button class:active-page={item === bucketPage} class="page-number" type="button" aria-label={`Go to bucket page ${item}`} aria-current={item === bucketPage ? 'page' : undefined} disabled={bucketsLoading} on:click={() => onBucketPage(item)}>{item}</button>{/if}{/each}</div><button class="pagination-arrow" type="button" aria-label="Next bucket page" disabled={bucketPage >= bucketPageCount || bucketsLoading} on:click={() => onBucketPage(bucketPage + 1)}>→</button><span class="pagination-summary">{bucketTotal} buckets</span></nav>{/if}
    {#if bucketSearch}<section class="global-search-card" aria-label="Recursive file search"><div class="global-search-head"><div><p class="card-label">Recursive file search</p><h3>Files in all buckets</h3></div><span class="search-count">{globalSearchTotal} {globalSearchTotal === 1 ? 'match' : 'matches'}</span></div>{#if globalSearchLoading}<div class="skeleton-stack" aria-label="Searching files"><div class="skeleton" style="height:52px"></div><div class="skeleton" style="height:52px"></div></div>{:else if globalSearchError}<LoadError title="Could not search files" message={globalSearchError} onRetry={onRefresh} />{:else if globalSearchResults.length === 0}<p class="empty-state compact-empty"><span class="empty-mark" aria-hidden="true">⌕</span>No files match this search across the buckets.</p>{:else}<ul class="global-search-list">{#each globalSearchResults as result (result.bucket + result.key)}<li><div class="global-result-copy"><strong>{result.name}</strong><small>{result.bucket}{result.location ? ` / ${result.location}` : ' / root'}</small></div><button class="ghost result-open" type="button" on:click={() => onOpenSearchResult(result)}>Open location</button></li>{/each}</ul>{#if globalSearchTotal > 25}<nav class="pagination" aria-label="Recursive search pages"><button class="pagination-arrow" type="button" aria-label="Previous search page" disabled={globalSearchPage <= 1 || globalSearchLoading} on:click={() => onGlobalSearchPage(globalSearchPage - 1)}>←</button><div class="page-numbers">{#each globalSearchPageItems as item}{#if item === 'ellipsis'}<span class="pagination-ellipsis" aria-hidden="true">…</span>{:else}<button class:active-page={item === globalSearchPage} class="page-number" type="button" aria-label={`Go to search page ${item}`} aria-current={item === globalSearchPage ? 'page' : undefined} disabled={globalSearchLoading} on:click={() => onGlobalSearchPage(item)}>{item}</button>{/if}{/each}</div><button class="pagination-arrow" type="button" aria-label="Next search page" disabled={globalSearchPage >= globalSearchPageCount || globalSearchLoading} on:click={() => onGlobalSearchPage(globalSearchPage + 1)}>→</button><span class="pagination-summary">{globalSearchTotal} matches</span></nav>{/if}{/if}</section>{/if}
  {:else}
    {#if listing}
      <div class="listing-frame" class:loading={objectsLoading} aria-busy={objectsLoading}>
        <div class="browser-toolbar object-toolbar">
          <label class="search-field search-wide">
            <span class="visually-hidden">Search objects</span>
            <input value={objectSearch} type="search" placeholder="Search this bucket…" aria-label="Search objects and folders" on:input={(event) => onObjectSearch((event.currentTarget as HTMLInputElement).value)} />
          </label>
          {#if objectSearch}<span class="search-hint">Searching recursively</span><button class="btn-link clear-search" type="button" on:click={() => onObjectSearch('')}>Clear search</button>{:else}<span class="search-hint">Browse this folder</span>{/if}
        </div>
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
            {#if obj.location}<button class="location-link" type="button" on:click={() => onGoToFolder(obj.location ?? '')}>in {obj.location}</button>{/if}
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
              <td><div class="row-actions">{#if obj.uploading}<span class="uploading-actions" class:needs-action={uploadKind(obj) === 'attention'} title={uploadTitle(obj)}><span aria-hidden="true"></span>{uploadKind(obj) === 'attention' ? 'Needs action' : 'Working'}</span>{:else if obj.rechunking}<span class="uploading-actions needs-action" title="This object is temporarily unavailable while its chunks are being rebuilt"><span aria-hidden="true"></span>Re-chunking · try again later</span>{:else}{#if (obj.replica_accounts ?? 0) + (obj.access_accounts ?? 0)}<ActionIcon name="accounts" tone="success" badge={(obj.replica_accounts ?? 0) + (obj.access_accounts ?? 0)} label={`Show account copies and access for ${obj.name}`} on:click={() => onOpenReplicas(selectedBucket, obj.key)}/>{/if}{#if obj.location}<ActionIcon name="folder" label={`Go to folder ${obj.location}`} on:click={() => onGoToFolder(obj.location ?? '')}/>{/if}<ActionIcon name="download" label={`Download ${obj.name}`} href={contentUrl(selectedBucket, obj.key)}/><ActionIcon name="share" label={`Share ${obj.name}`} on:click={() => onShare(obj)} disabled={busy}/><ActionIcon name="links" badge={obj.shared_links} label={`Manage shared links for ${obj.name}`} on:click={() => onOpenShareLinks(obj)} disabled={busy}/><ActionIcon name="trash" label={`Delete ${obj.name}`} tone="danger" on:click={() => onRemoveKey(obj)} disabled={busy}/>{/if}</div></td>
        </tr>
      {/each}
    </tbody></table></div>{/if}
      </div>
      {#if objectTotal > 25}<nav class="pagination" aria-label="Object pages"><button class="pagination-arrow" type="button" aria-label="Previous object page" disabled={objectPage <= 1 || objectsLoading} on:click={() => onObjectPage(objectPage - 1)}>←</button><div class="page-numbers">{#each objectPageItems as item}{#if item === 'ellipsis'}<span class="pagination-ellipsis" aria-hidden="true">…</span>{:else}<button class:active-page={item === objectPage} class="page-number" type="button" aria-label={`Go to object page ${item}`} aria-current={item === objectPage ? 'page' : undefined} disabled={objectsLoading} on:click={() => onObjectPage(item)}>{item}</button>{/if}{/each}</div><button class="pagination-arrow" type="button" aria-label="Next object page" disabled={objectPage >= objectPageCount || objectsLoading} on:click={() => onObjectPage(objectPage + 1)}>→</button><span class="pagination-summary">{objectTotal} items</span></nav>{/if}
    {:else if objectsLoading}<div class="skeleton-stack" aria-label="Loading files"><div class="skeleton" style="height:40px"></div><div class="skeleton" style="height:40px"></div><div class="skeleton" style="height:40px"></div></div>
    {:else if objectsError}<LoadError title="Could not load this folder" message={objectsError} onRetry={onRefresh} />
    {/if}
    {#if selectedKeys.length}<div class="selection-bar"><strong>{selectedKeys.length} selected</strong><button class="ghost bulk-action bulk-replicate" type="button" on:click={onOpenBulkReplication}><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="9" cy="8" r="3"/><path d="M3.5 19a5.5 5.5 0 0 1 11 0M16 11a3 3 0 0 1 4.5 2.6M16.5 19a4 4 0 0 1 4 0"/></svg><span>Replicate</span></button><button class="ghost bulk-action bulk-delete" type="button" on:click={onRemoveSelected}><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h16m-10 4v6m4-6v6M9 7V4h6v3m-9 0 1 13h10l1-13"/></svg><span>Delete</span></button><button class="ghost bulk-action" type="button" on:click={onOpenMove}><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3v18m0-18-3 3m3-3 3 3M12 21l-3-3m3 3 3-3M3 12h18m0 0-3-3m3 3-3 3M3 12l3-3m-3 3 3 3"/></svg><span>Move</span></button><button class="ghost bulk-action" type="button" on:click={onRechunkSelected}><svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 6h16M4 12h16M4 18h16M8 4v4m8 2v4m-5 2v4"/></svg><span>Re-chunk</span></button></div>{/if}
  {/if}
</section>
<style>
  .heading-row { display: flex; align-items: center; gap: .75rem; }
  .back-button { flex: 0 0 auto; }
  .listing-frame { position: relative; min-height: 72px; }
  .browser-toolbar { display: flex; align-items: center; gap: 12px; margin: 14px 0; }
  .object-toolbar { margin-top: 0; padding: 10px 0 12px; border-bottom: 1px solid #e2ebf2; }
  .search-field { display: block; flex: 1 1 360px; max-width: 520px; }
  .search-field input { width: 100%; min-height: 40px; padding: 9px 13px; border: 1px solid var(--border); border-radius: 10px; background: #fff; color: var(--ink); box-sizing: border-box; }
  .search-field input:focus { outline: 3px solid rgba(43,130,197,.16); border-color: #68a9d2; }
  .search-hint { color: var(--muted); font-size: .75rem; }
  .clear-search { flex: 0 0 auto; font-size: .78rem; }
  .pagination { display: flex; align-items: center; justify-content: center; gap: 10px; margin-top: 18px; color: var(--muted); font-size: .78rem; }
  .page-numbers { display: flex; align-items: center; gap: 5px; }
  .pagination-arrow, .page-number { display: inline-grid; place-items: center; min-width: 34px; height: 34px; padding: 0 8px; border: 1px solid #d8e3eb; border-radius: 10px; background: #fff; color: #3f6077; font: inherit; font-weight: 750; cursor: pointer; transition: border-color 150ms ease, background 150ms ease, color 150ms ease, box-shadow 150ms ease; }
  .pagination-arrow { font-size: 1rem; }
  .pagination-arrow:hover:not(:disabled), .page-number:hover:not(:disabled) { border-color: #8bb9d7; background: #f4fbff; color: var(--accent); }
  .page-number.active-page { border-color: var(--accent); background: var(--accent); color: #fff; box-shadow: 0 5px 12px rgba(43,130,197,.2); }
  .pagination-arrow:disabled, .page-number:disabled { cursor: not-allowed; opacity: .45; }
  .pagination-ellipsis { display: inline-grid; place-items: center; min-width: 20px; color: #8aa0b0; font-weight: 800; }
  .pagination-summary { margin-left: 6px; white-space: nowrap; }
  .global-search-card { margin-top: 20px; padding: 18px; border: 1px solid #dbe8f0; border-radius: 14px; background: linear-gradient(145deg, #fbfdff, #f4faff); }
  .global-search-head { display: flex; align-items: center; justify-content: space-between; gap: 16px; margin-bottom: 12px; }
  .global-search-head h3 { margin: 3px 0 0; color: #203b57; font-size: 1rem; }
  .search-count { color: #2b82c5; font-size: .75rem; font-weight: 800; white-space: nowrap; }
  .global-search-list { display: grid; gap: 8px; margin: 0; padding: 0; list-style: none; }
  .global-search-list li { display: flex; align-items: center; justify-content: space-between; gap: 16px; padding: 11px 12px; border: 1px solid #e1edf4; border-radius: 11px; background: #fff; }
  .global-result-copy { display: grid; gap: 3px; min-width: 0; }
  .global-result-copy strong { overflow-wrap: anywhere; color: #203b57; }
  .global-result-copy small { overflow-wrap: anywhere; color: var(--muted); font-size: .72rem; }
  .result-open { flex: 0 0 auto; padding: 7px 10px; }
  .compact-empty { margin: 0; padding: 22px 0 10px; }
  .location-link { display: block; max-width: 100%; overflow: hidden; padding: 2px 0; border: 0; background: transparent; color: #49779a; font-size: .72rem; text-align: left; text-overflow: ellipsis; white-space: nowrap; cursor: pointer; }
  .location-link:hover { color: var(--accent); text-decoration: underline; }
  .listing-status { display: flex; align-items: center; justify-content: flex-end; gap: 8px; min-height: 34px; margin: 0 0 8px; padding: 7px 10px; border: 1px solid #cfe4f1; border-radius: 10px; background: #f3faff; color: #2d6789; font-size: .78rem; font-weight: 750; }
  .listing-status.error { justify-content: space-between; gap: 12px; border-color: #f0c9c9; background: #fff7f7; color: var(--danger, #b00020); }
  .listing-status .ghost { flex: 0 0 auto; padding: 5px 10px; }
  .listing-frame .table-scroll { transition: opacity 180ms ease, filter 180ms ease; }
  .listing-frame .listing-dimmed { opacity: .48; filter: saturate(.7); pointer-events: none; }
  .bucket-row { display: flex; align-items: center; justify-content: space-between; gap: 1rem; }
  .bucket-meta { display:flex; align-items:center; gap:8px; }
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
  .selection-bar { position: fixed; left: calc(230px + clamp(20px, 3vw, 40px)); right: clamp(20px, 3vw, 40px); bottom: 18px; z-index: 15; display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 12px 14px; border: 1px solid #c9ddeb; border-radius: 16px; background: color-mix(in srgb, var(--surface) 92%, #dff1ff); box-shadow: 0 18px 40px rgba(20,55,90,.2); backdrop-filter: blur(14px); }
  .selection-bar .bulk-action { display: inline-flex; align-items: center; gap: 6px; }
  .selection-bar .bulk-action svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
  .selection-bar .bulk-delete { color: var(--danger, #b00020); }
  .selection-bar .bulk-replicate { color: #17604a; }
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
  @media (max-width: 700px) { .browser-toolbar { align-items: stretch; flex-wrap: wrap; } .search-field { flex-basis: 100%; max-width: none; } .object-toolbar .search-hint { flex: 1 1 auto; } .pagination { flex-wrap: wrap; } .pagination-summary { flex-basis: 100%; margin: 0; text-align: center; } .global-search-head, .global-search-list li { align-items: stretch; flex-direction: column; } .result-open { align-self: flex-start; } }
  @media (max-width: 900px) { .selection-bar { left: 12px; right: 12px; bottom: 12px; } }
  .visually-hidden { position:absolute!important; width:1px!important; height:1px!important; padding:0!important; margin:-1px!important; overflow:hidden!important; clip:rect(0,0,0,0)!important; white-space:nowrap!important; border:0!important; }
</style>
