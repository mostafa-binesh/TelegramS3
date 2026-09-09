<script lang="ts">
  import { formatCount } from '../lib/format';
  import type { OverviewState } from '../lib/types';

  export let overview: OverviewState | null;
  export let loading = false;
  export let corruptedCount = 0;
  export let acknowledgedCount = 0;
  export let onRefresh: () => void = () => {};
  export let onRecovery: () => void = () => {};
</script>

<section class="section-head overview-head">
  <div><p class="card-label">Snapshot</p><h2>Storage at a glance</h2></div>
  <button class="ghost" type="button" on:click={onRefresh} disabled={loading}>
    {#if loading}<span class="spinner" aria-hidden="true"></span>{/if}Refresh
  </button>
</section>

<style>
  .overview-head { margin-bottom: .25rem; }
  .overview-head h2 { margin: .25rem 0 0; }
  .analysis-grid { grid-template-columns: 1.25fr .75fr; }
  .chart-card h2 { margin: .25rem 0 1.2rem; }
  .bar-chart { height: 22px; display: flex; overflow: hidden; border-radius: 999px; background: #edf1f5; }
  .bar-segment { min-width: 0; }
  .bar-segment.committed, .legend .committed { background: #2779bc; }
  .bar-segment.active, .legend .active { background: #58a37c; }
  .bar-segment.staged, .legend .staged { background: #d59a47; }
  .legend { display: flex; flex-wrap: wrap; gap: 10px 18px; margin-top: 14px; color: var(--muted); font-size: 12px; }
  .legend span { display: inline-flex; align-items: center; gap: 6px; }
  .legend i { display: inline-block; width: 8px; height: 8px; border-radius: 50%; }
  .signal-track { height: 10px; border-radius: 99px; background: #edf1f5; overflow: hidden; }
  .signal-track span { display: block; height: 100%; background: #d59a47; border-radius: inherit; }
  .corrupted { display: flex; flex-direction: column; align-items: flex-start; }
  .corrupted small { display: block; margin-top: .35rem; color: var(--muted); }
  .corrupted.attention { border-color: color-mix(in srgb, var(--danger) 40%, var(--border)); background: color-mix(in srgb, var(--danger) 5%, var(--surface)); }
  .corrupted.attention strong { color: var(--danger); }
  .card-link { margin-top: .6rem; font-size: .85rem; }
  @media (max-width: 760px) { .analysis-grid { grid-template-columns: 1fr; } }
</style>
<section class="cards">
  {#if loading || !overview}
    {#each [0, 1, 2, 3] as slot (slot)}<article class="card metric"><div class="skeleton" style="height:62px"></div></article>{/each}
  {:else}
    <article class="card metric"><p class="card-label">Buckets</p><strong>{formatCount(overview.storage?.buckets ?? 0)}</strong></article>
    <article class="card metric"><p class="card-label">Committed</p><strong>{formatCount(overview.storage?.committed_objects ?? 0)}</strong></article>
    <article class="card metric"><p class="card-label">Active</p><strong>{formatCount(overview.storage?.active_objects ?? 0)}</strong></article>
    <article class="card metric corrupted" class:attention={corruptedCount > 0}>
      <p class="card-label">Corrupted files</p><strong>{formatCount(corruptedCount)}</strong>
      {#if overview.recovery?.scan_error}<small class="error-hint">Scan unavailable</small>
      {:else if acknowledgedCount > 0}<small>{formatCount(acknowledgedCount)} acknowledged</small>
      {:else if corruptedCount === 0}<small>Nothing needs attention</small>{/if}
      {#if corruptedCount > 0 || acknowledgedCount > 0 || overview.recovery?.scan_error}<button class="btn-link card-link" type="button" on:click={onRecovery}>View details →</button>{/if}
    </article>
  {/if}
</section>
<section class="layout analysis-grid">
  <article class="card surface chart-card">
    <div class="section-head"><div><p class="card-label">Analysis</p><h2>Storage composition</h2></div></div>
    {#if loading || !overview}<div class="skeleton" style="height:150px"></div>{:else}
      {@const total = Math.max((overview.storage?.committed_objects ?? 0) + (overview.storage?.active_objects ?? 0) + (overview.storage?.staged_objects ?? 0), 1)}
      <div class="bar-chart" aria-label="Storage composition chart"><div class="bar-segment committed" style={`width:${((overview.storage?.committed_objects ?? 0) / total) * 100}%`}></div><div class="bar-segment active" style={`width:${((overview.storage?.active_objects ?? 0) / total) * 100}%`}></div><div class="bar-segment staged" style={`width:${((overview.storage?.staged_objects ?? 0) / total) * 100}%`}></div></div>
      <div class="legend"><span><i class="committed"></i>Committed {formatCount(overview.storage?.committed_objects ?? 0)}</span><span><i class="active"></i>Active {formatCount(overview.storage?.active_objects ?? 0)}</span><span><i class="staged"></i>Staged {formatCount(overview.storage?.staged_objects ?? 0)}</span></div>
    {/if}
  </article>
  <article class="card surface chart-card"><p class="card-label">Recovery signal</p><h2>{formatCount(corruptedCount)} actionable</h2>
    {#if loading || !overview}<div class="skeleton" style="height:80px"></div>{:else}<div class="signal-track"><span style={`width:${Math.min(corruptedCount * 10, 100)}%`}></span></div><p class="fine-print">{acknowledgedCount ? `${formatCount(acknowledgedCount)} acknowledged issue(s) remain reviewable.` : 'No acknowledged issues.'}</p>{/if}
  </article>
</section>
