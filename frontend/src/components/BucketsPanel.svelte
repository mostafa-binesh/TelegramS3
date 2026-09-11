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

  $: allVisibleSelected = selectedKeys.length > 0 && selectedKeys.length === (listing?.objects.length ?? 0);
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
    {#if objectsLoading}<div class="skeleton-stack" aria-label="Loading files"><div class="skeleton" style="height:40px"></div><div class="skeleton" style="height:40px"></div><div class="skeleton" style="height:40px"></div></div>
    {:else if objectsError}<LoadError title="Could not load this folder" message={objectsError} onRetry={onRefresh} />
    {:else if listing && listing.folders.length === 0 && listing.objects.length === 0}<p class="empty-state"><span class="empty-mark" aria-hidden="true">↑</span>This folder is empty. Drop files above to upload the first one.</p>
    {:else}<div class="table-scroll"><table class="kv-table"><colgroup><col class="selection-column"/><col class="name-column"/><col class="size-column"/><col class="modified-column"/><col class="actions-column"/></colgroup><thead><tr><th><input class="select-all" type="checkbox" aria-label="Select all visible items" checked={allVisibleSelected} on:change={onToggleAll}/></th><th>Name</th><th>Size</th><th>Modified</th><th><span class="visually-hidden">Actions</span></th></tr></thead><tbody>
      {#each listing?.folders ?? [] as folder (folder)}<tr><td></td><td><button class="btn-link" on:click={() => onEnterFolder(folder)}>{folder}/</button></td><td class="muted">folder</td><td class="muted">—</td><td class="row-actions"><ActionIcon name="trash" label={`Delete folder ${folder}`} tone="danger" on:click={() => onRemoveKey(folder)} disabled={busy}/></td></tr>{/each}
      {#each listing?.objects ?? [] as obj (obj.key)}<tr><td><input class="select-all" type="checkbox" checked={selectedKeys.includes(obj.key)} on:change={() => onToggleKey(obj.key)} aria-label={`Select ${obj.name}`}/></td><td>{obj.name}{#if obj.expires_at}<small class="expiry-note">expires {formatTimestamp(obj.expires_at)}</small>{/if}</td><td>{formatBytes(obj.size)}</td><td>{formatTimestamp(obj.last_modified)}</td><td class="row-actions"><ActionIcon name="download" label={`Download ${obj.name}`} href={contentUrl(selectedBucket, obj.key)}/><ActionIcon name="share" label={`Share ${obj.name}`} on:click={() => onShare(obj)} disabled={busy}/><ActionIcon name="links" badge={obj.shared_links} label={`Manage shared links for ${obj.name}`} on:click={() => onOpenShareLinks(obj)} disabled={busy}/><ActionIcon name="trash" label={`Delete ${obj.name}`} tone="danger" on:click={() => onRemoveKey(obj)} disabled={busy}/></td></tr>{/each}
    </tbody></table></div>{/if}
    {#if selectedKeys.length}<div class="selection-bar"><strong>{selectedKeys.length} selected</strong><button class="ghost" on:click={onRemoveSelected}>Delete</button><button class="ghost" on:click={onOpenMove}>→ Move</button></div>{/if}
  {/if}
</section>
<style>
  .heading-row { display: flex; align-items: center; gap: .75rem; }
  .back-button { flex: 0 0 auto; }
  .bucket-row { display: flex; align-items: center; justify-content: space-between; gap: 1rem; }
  .expiry-note { display:block; color:var(--muted); font-size:.75rem; }
  .kv-table { table-layout: fixed; min-width: 800px; }
  .selection-column { width: 44px; }
  .size-column { width: 120px; }
  .modified-column { width: 180px; }
  .actions-column { width: 194px; }
  .kv-table th, .kv-table td { vertical-align: middle; }
  .kv-table th:nth-child(2), .kv-table td:nth-child(2) { overflow-wrap: anywhere; }
  .row-actions { justify-content: flex-end; }
  .row-actions :global(.action-icon) { flex: 0 0 38px; }
  .visually-hidden { position:absolute!important; width:1px!important; height:1px!important; padding:0!important; margin:-1px!important; overflow:hidden!important; clip:rect(0,0,0,0)!important; white-space:nowrap!important; border:0!important; }
</style>
