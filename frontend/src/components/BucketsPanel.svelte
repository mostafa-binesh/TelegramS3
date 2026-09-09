<script lang="ts">
  import { contentUrl } from '../lib/api';
  import { formatBytes, formatTimestamp } from '../lib/format';
  import type { BucketInfo, ObjectEntry, ObjectsState } from '../lib/types';

  export let buckets: BucketInfo[] = [];
  export let selectedBucket = '';
  export let listing: ObjectsState | null = null;
  export let bucketsLoading = false;
  export let objectsLoading = false;
  export let busy = false;
  export let selectedKeys: string[] = [];
  export let onCreateBucket: () => void = () => {};
  export let onRefresh: () => void = () => {};
  export let onUpload: () => void = () => {};
  export let onOpenBucket: (name: string) => void = () => {};
  export let onEnterFolder: (name: string) => void = () => {};
  export let onOpenFolder: () => void = () => {};
  export let onToggleKey: (key: string) => void = () => {};
  export let onToggleAll: () => void = () => {};
  export let onRemoveKey: (object: ObjectEntry | string) => void = () => {};
  export let onRemoveSelected: () => void = () => {};
  export let onOpenMove: () => void = () => {};

  $: allVisibleSelected = selectedKeys.length > 0 && selectedKeys.length === (listing?.objects.length ?? 0);
</script>

<section class="card surface">
  <div class="section-head"><div><p class="card-label">Buckets and files</p><h2>{selectedBucket ? `Bucket / ${selectedBucket}` : 'Your buckets'}</h2></div>
    <div class="toolbar-actions">
      {#if !selectedBucket}<button class="primary" type="button" on:click={onCreateBucket}>＋ Create bucket</button>{/if}
      <button class="ghost" type="button" on:click={onRefresh} disabled={busy || bucketsLoading || objectsLoading}>{#if bucketsLoading || objectsLoading}<span class="spinner" aria-hidden="true"></span>{/if}Refresh</button>
      {#if selectedBucket}<button class="primary" type="button" on:click={onUpload}>↑ Upload</button>{/if}
    </div>
  </div>
  {#if !selectedBucket}
    <p class="fine-print">Select a bucket to browse its files. Bucket names may contain Unicode characters.</p>
    {#if bucketsLoading && buckets.length === 0}<div class="skeleton-stack"><div class="skeleton" style="height:52px"></div><div class="skeleton" style="height:52px"></div></div>
    {:else if buckets.length === 0}<p class="empty-state"><span class="empty-mark" aria-hidden="true">+</span>No buckets yet. Create one above to start the file browser.</p>
    {:else}<ul class="checks">{#each buckets as bucket (bucket.name)}<li><div class="bucket-row"><button type="button" class="btn-link" on:click={() => onOpenBucket(bucket.name)}>{bucket.name}<small>created {formatTimestamp(bucket.created_at)}</small></button><span class="fine-print">{bucket.name.length} chars</span></div></li>{/each}</ul>{/if}
  {:else}
    <div class="row-inline folder-actions"><button class="ghost" on:click={onOpenFolder}>＋ New folder</button></div>
    {#if objectsLoading && !listing}<div class="skeleton-stack"><div class="skeleton" style="height:40px"></div><div class="skeleton" style="height:40px"></div><div class="skeleton" style="height:40px"></div></div>
    {:else if listing && listing.folders.length === 0 && listing.objects.length === 0}<p class="empty-state"><span class="empty-mark" aria-hidden="true">↑</span>This folder is empty. Drop files above to upload the first one.</p>
    {:else}<div class="table-scroll"><table class="kv-table"><thead><tr><th><input class="select-all" type="checkbox" aria-label="Select all visible items" checked={allVisibleSelected} on:change={onToggleAll}/></th><th>Name</th><th>Size</th><th>Modified</th><th></th></tr></thead><tbody>
      {#each listing?.folders ?? [] as folder (folder)}<tr><td></td><td><button class="btn-link" on:click={() => onEnterFolder(folder)}>{folder}/</button></td><td class="muted">folder</td><td class="muted">—</td><td class="row-actions"><button class="ghost" on:click={() => onRemoveKey(folder)}>Delete</button></td></tr>{/each}
      {#each listing?.objects ?? [] as obj (obj.key)}<tr><td><input class="select-all" type="checkbox" checked={selectedKeys.includes(obj.key)} on:change={() => onToggleKey(obj.key)} aria-label={`Select ${obj.name}`}/></td><td>{obj.name}</td><td>{formatBytes(obj.size)}</td><td>{formatTimestamp(obj.last_modified)}</td><td class="row-actions"><a class="row-download" href={contentUrl(selectedBucket, obj.key)} download>Download</a><button class="ghost" on:click={() => onRemoveKey(obj)}>Delete</button></td></tr>{/each}
    </tbody></table></div>{/if}
    {#if selectedKeys.length}<div class="selection-bar"><strong>{selectedKeys.length} selected</strong><button class="ghost" on:click={onRemoveSelected}>Delete</button><button class="ghost" on:click={onOpenMove}>→ Move</button></div>{/if}
  {/if}
</section>
