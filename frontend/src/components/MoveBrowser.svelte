<script lang="ts">
  import { listBuckets, listObjects } from '../lib/api';
  import type { BucketInfo, ObjectsState } from '../lib/types';
  import { onMount } from 'svelte';
  import LoadError from './LoadError.svelte';

  export let bucket = '';
  export let prefix = '';
  export let csrf: string | null | undefined;
  export let busy = false;
  export let onMoveHere: () => void = () => {};

  let buckets: BucketInfo[] = [];
  let listing: ObjectsState | null = null;
  let loadingBuckets = true;
  let loadingObjects = false;
  let error = '';
  let loadedLocation = '';

  $: locationKey = `${bucket}|${prefix}`;
  $: if (bucket && locationKey !== loadedLocation) {
    loadedLocation = locationKey;
    void loadObjects(bucket, prefix);
  }

  async function loadBuckets() {
    loadingBuckets = true;
    try {
      buckets = (await listBuckets(csrf)).buckets ?? [];
      error = '';
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Unable to load buckets';
    } finally {
      loadingBuckets = false;
    }
  }

  async function loadObjects(nextBucket: string, nextPrefix: string) {
    loadingObjects = true;
    listing = null;
    error = '';
    try {
      listing = await listObjects(csrf, nextBucket, nextPrefix);
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Unable to load folders';
    } finally {
      loadingObjects = false;
    }
  }

  function chooseBucket(name: string) {
    bucket = name;
    prefix = '';
  }

  function enterFolder(name: string) {
    prefix = `${prefix}${name}/`;
  }

  function goUp() {
    const parts = prefix.split('/').filter(Boolean);
    parts.pop();
    prefix = parts.length ? `${parts.join('/')}/` : '';
  }

  function gotoCrumb(index: number) {
    const parts = prefix.split('/').filter(Boolean).slice(0, index);
    prefix = parts.length ? `${parts.join('/')}/` : '';
  }

  function destination() {
    return `${bucket}/${prefix}`;
  }

  onMount(() => { void loadBuckets(); });
</script>

<div class="move-browser">
  <div class="move-browser-columns">
    <section class="move-browser-list" aria-label="Destination buckets">
      <p class="card-label">Buckets</p>
      {#if loadingBuckets}<div class="skeleton-stack"><div class="skeleton" style="height:38px"></div><div class="skeleton" style="height:38px"></div></div>
      {:else if error && !buckets.length}<LoadError title="Could not load destination buckets" message={error} onRetry={loadBuckets} />
      {:else}{#each buckets as item (item.name)}<button class:chosen={bucket === item.name} class="move-option" type="button" on:click={() => chooseBucket(item.name)}>{item.name}</button>{/each}{/if}
    </section>
    <section class="move-browser-list" aria-label="Destination folders">
      <div class="move-browser-head"><p class="card-label">Folders</p>{#if bucket && prefix}<button class="btn-link" type="button" on:click={goUp}>↑ Up</button>{/if}</div>
      {#if bucket}
        <div class="move-crumbs"><button class="btn-link" type="button" on:click={() => gotoCrumb(0)}>{bucket}</button>{#each prefix.split('/').filter(Boolean) as crumb, i (crumb + i)}<span>/</span><button class="btn-link" type="button" on:click={() => gotoCrumb(i + 1)}>{crumb}</button>{/each}</div>
        {#if loadingObjects}<div class="skeleton-stack"><div class="skeleton" style="height:38px"></div><div class="skeleton" style="height:38px"></div><div class="skeleton" style="height:38px"></div></div>
        {:else if error && !listing}<LoadError title="Could not load destination folders" message={error} onRetry={() => loadObjects(bucket, prefix)} />
        {:else if listing}{#each listing.folders as folder (folder)}<button class="move-option" type="button" on:click={() => enterFolder(folder)}>📁 {folder}/</button>{/each}{#each listing.objects as object (object.key)}<div class="move-option disabled">{object.name}<small>object</small></div>{/each}{#if !listing.folders.length && !listing.objects.length}<p class="fine-print">This folder is empty.</p>{/if}{/if}
      {:else}<p class="fine-print">Choose a bucket to browse folders.</p>{/if}
    </section>
  </div>
  {#if error && (buckets.length > 0 || listing)}<p class="fine-print error-hint" role="alert">{error}</p>{/if}
  <div class="move-destination"><span>Move here:</span><strong>{bucket ? destination() : 'Choose a bucket'}</strong><button class="primary" type="button" on:click={onMoveHere} disabled={!bucket || loadingObjects || busy}>{#if busy}<span class="spinner" aria-hidden="true"></span>{/if}{busy ? 'Moving…' : 'Move files'}</button></div>
</div>

<style>
  .move-browser{display:grid;gap:14px}.move-browser-columns{display:grid;grid-template-columns:minmax(150px,.8fr) minmax(0,1.2fr);gap:12px}.move-browser-list{min-width:0;border:1px solid var(--border);border-radius:var(--radius-md);padding:12px}.move-browser-head,.move-destination{display:flex;align-items:center;justify-content:space-between;gap:10px}.move-option{width:100%;justify-content:flex-start;background:transparent;color:var(--text);border:1px solid transparent;padding:.62rem .7rem;text-align:left}.move-option:hover:not(:disabled),.move-option.chosen{background:var(--accent-soft);border-color:var(--accent-ring)}.move-option.disabled{color:var(--muted);cursor:not-allowed;opacity:.65}.move-option small{display:block;font-size:.75rem}.move-crumbs{display:flex;gap:5px;align-items:center;flex-wrap:wrap;margin:8px 0;font-size:.82rem}.move-destination{padding-top:4px;flex-wrap:wrap}.move-destination strong{overflow-wrap:anywhere;margin-right:auto}@media(max-width:620px){.move-browser-columns{grid-template-columns:1fr}}
</style>
