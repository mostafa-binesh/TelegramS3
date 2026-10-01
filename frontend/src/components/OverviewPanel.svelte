<script lang="ts">
  import { onMount } from 'svelte';
  import { formatBytes, formatCount } from '../lib/format';
  import LoadError from './LoadError.svelte';
  import type { OverviewState } from '../lib/types';

  export let overview: OverviewState | null;
  export let loading = false;
  export let error = '';
  export let corruptedCount = 0;
  export let acknowledgedCount = 0;
  export let onRefresh: () => void = () => {};
  export let onRecovery: () => void = () => {};
  export let onStageMetricsTest: () => void = () => {};
  export let stageTestBusy = false;

  const emptyTransferMetrics = {
    pending_jobs: 0,
    oldest_pending_age_seconds: 0,
    retries: 0,
    failed_jobs: 0,
    staging_bytes: 0,
    cleanup_backlog: 0,
    cleanup_due: 0,
    cleanup_scheduled: 0,
    cleanup_recovery_required: 0
  };

  const emptyTrafficMetrics = {
    session: {
      client_upload_bytes: 0,
      client_download_bytes: 0,
      telegram_upload_bytes: 0,
      telegram_download_bytes: 0
    },
    total: {
      client_upload_bytes: 0,
      client_download_bytes: 0,
      telegram_upload_bytes: 0,
      telegram_download_bytes: 0
    }
  };

  const emptyStageMetrics = {
    active_requests: 0,
    completed_requests: 0,
    failed_requests: 0,
    test_active_requests: 0,
    last_test: null,
    recent: []
  };

  function formatAge(seconds: number) {
    if (!seconds) return 'No pending work';
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m oldest`;
    const hours = Math.floor(minutes / 60);
    const remainder = minutes % 60;
    return `${hours}h ${remainder}m oldest`;
  }

  $: transferMetrics = { ...emptyTransferMetrics, ...(overview?.transfers ?? {}) };
  $: trafficMetrics = overview?.traffic ?? emptyTrafficMetrics;
  $: stageMetrics = overview?.stage_metrics ?? emptyStageMetrics;
  let trafficTab: 'session' | 'total' = 'session';
  $: trafficView = trafficMetrics[trafficTab];
  $: checks = overview?.checks ?? [];
  $: passingChecks = checks.filter((check) => check.ok).length;
  let now = Date.now();
  onMount(() => {
    const timer = window.setInterval(() => now = Date.now(), 1000);
    return () => window.clearInterval(timer);
  });

  function formatCountdown(value?: string | null, enabled = true) {
    if (!enabled) return 'Disabled';
    if (!value) return 'Waiting for first scan';
    const remaining = Math.floor((Date.parse(value) - now) / 1000);
    if (remaining <= 0) return 'Due now';
    const minutes = Math.floor(remaining / 60);
    const seconds = remaining % 60;
    if (minutes >= 60) return `in ${Math.floor(minutes / 60)}h ${minutes % 60}m`;
    return `in ${minutes}m ${seconds.toString().padStart(2, '0')}s`;
  }

  function verifierStatusLabel(status: string) {
    return status === 'disabled' ? 'Disabled' : status === 'healthy' ? 'Healthy' : status === 'attention' ? 'Needs attention' : status === 'unavailable' ? 'Unavailable' : status === 'scheduled' ? 'Scheduled' : 'Starting';
  }

  function formatStageDuration(microseconds?: number | null) {
    if (microseconds == null) return '—';
    if (microseconds < 1000) return `${microseconds}µs`;
    const milliseconds = microseconds / 1000;
    if (milliseconds < 1000) return `${milliseconds < 10 ? milliseconds.toFixed(1) : Math.round(milliseconds)}ms`;
    return `${(milliseconds / 1000).toFixed(1)}s`;
  }

  function accountLabel(accountId: string) {
    return overview?.telegram?.accounts?.find((account) => account.id === accountId)?.label ?? accountId;
  }
</script>

<section class="section-head overview-head">
  <div><p class="card-label">Snapshot</p><h2>Storage at a glance</h2></div>
  <button class="ghost" type="button" on:click={onRefresh} disabled={loading}>
    {#if loading}<span class="spinner" aria-hidden="true"></span>{/if}Refresh
  </button>
</section>

{#if error && !overview}
  <LoadError title="Could not load the overview" message={error} onRetry={onRefresh} />
{:else}
  {#if error}
    <LoadError title="Overview refresh failed" message={`${error} Showing the last available snapshot.`} onRetry={onRefresh} />
  {/if}

  <section class="cards">
    {#if loading || !overview}
      {#each [0, 1, 2, 3, 4] as slot (slot)}<article class="card metric"><div class="skeleton" style="height:62px"></div></article>{/each}
    {:else}
      <article class="card metric"><p class="card-label">Buckets</p><strong>{formatCount(overview.storage?.buckets ?? 0)}</strong></article>
      <article class="card metric"><p class="card-label">Committed</p><strong>{formatCount(overview.storage?.committed_objects ?? 0)}</strong></article>
      <article class="card metric"><p class="card-label">Active</p><strong>{formatCount(overview.storage?.active_objects ?? 0)}</strong></article>
      <article class="card metric"><p class="card-label">Telegram files</p><strong>{formatBytes(overview.storage?.telegram_files_bytes ?? 0)}</strong><small>committed remote payload</small></article>
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
  <section class="layout insight-grid">
    <article class="card surface analytics-card">
      <div class="analytics-heading"><div><p class="card-label">Live queue snapshot</p><h2>Transfer pipeline</h2></div><span class:clear={transferMetrics.pending_jobs === 0 && transferMetrics.failed_jobs === 0 && transferMetrics.cleanup_due === 0 && transferMetrics.cleanup_recovery_required === 0} class="score-chip">{transferMetrics.failed_jobs || transferMetrics.cleanup_recovery_required ? 'Needs attention' : transferMetrics.pending_jobs || transferMetrics.cleanup_due ? 'In progress' : 'Clear'}</span></div>
      {#if loading || !overview}<div class="skeleton" style="height:132px"></div>{:else}
        {@const cleanupTotal = transferMetrics.cleanup_due + transferMetrics.cleanup_scheduled + transferMetrics.cleanup_recovery_required}
        {@const pipelineTotal = Math.max(transferMetrics.pending_jobs + transferMetrics.failed_jobs + cleanupTotal, 1)}
        <div class="pipeline-chart" role="img" aria-label="Transfer pipeline chart"><span class="pipeline-segment pending" style={`width:${(transferMetrics.pending_jobs / pipelineTotal) * 100}%`}></span><span class="pipeline-segment failed" style={`width:${(transferMetrics.failed_jobs / pipelineTotal) * 100}%`}></span><span class="pipeline-segment cleanup-due" style={`width:${(transferMetrics.cleanup_due / pipelineTotal) * 100}%`}></span><span class="pipeline-segment cleanup-scheduled" style={`width:${(transferMetrics.cleanup_scheduled / pipelineTotal) * 100}%`}></span><span class="pipeline-segment cleanup-recovery" style={`width:${(transferMetrics.cleanup_recovery_required / pipelineTotal) * 100}%`}></span></div>
        <div class="legend pipeline-legend"><span><i class="pending"></i>Pending {formatCount(transferMetrics.pending_jobs)}</span><span><i class="failed"></i>Failed {formatCount(transferMetrics.failed_jobs)}</span><span><i class="cleanup-due"></i>Cleanup due {formatCount(transferMetrics.cleanup_due)}</span><span><i class="cleanup-scheduled"></i>Scheduled {formatCount(transferMetrics.cleanup_scheduled)}</span><span><i class="cleanup-recovery"></i>Recovery required {formatCount(transferMetrics.cleanup_recovery_required)}</span></div>
        <div class="metric-strip"><div><strong>{formatCount(transferMetrics.retries)}</strong><small>retry attempts</small></div><div><strong>{formatBytes(transferMetrics.staging_bytes)}</strong><small>staged locally</small></div><div><strong>{formatAge(transferMetrics.oldest_pending_age_seconds)}</strong><small>oldest pending</small></div></div>
      {/if}
    </article>
    <article class="card surface checks-card">
      <div class="analytics-heading"><div><p class="card-label">Readiness</p><h2>System checks</h2></div><span class:healthy={checks.length > 0 && passingChecks === checks.length} class="score-chip">{passingChecks}/{checks.length}</span></div>
      {#if loading || !overview}<div class="skeleton" style="height:132px"></div>{:else if checks.length === 0}<p class="fine-print">No health checks were reported in this snapshot.</p>{:else}<div class="check-list">{#each checks as check (check.label)}<div class="check-row"><span class:ok={check.ok} class="check-dot" aria-hidden="true"></span><div><strong>{check.label}</strong><small>{check.detail}</small></div><span class:ok={check.ok} class="check-state">{check.ok ? 'Ready' : 'Review'}</span></div>{/each}</div>{/if}
    </article>
    <article class="card surface traffic-card">
      <div class="analytics-heading"><div><p class="card-label">Network usage</p><h2>{trafficTab === 'session' ? 'Traffic since process start' : 'Traffic across all server runs'}</h2></div><span class="live-chip"><span aria-hidden="true"></span>Refreshes every 5s</span></div>
      {#if loading || !overview}<div class="skeleton" style="height:154px"></div>{:else}
        <div class="traffic-tabs" role="tablist" aria-label="Network usage period">
          <button class:active={trafficTab === 'session'} type="button" role="tab" aria-selected={trafficTab === 'session'} on:click={() => trafficTab = 'session'}>This session</button>
          <button class:active={trafficTab === 'total'} type="button" role="tab" aria-selected={trafficTab === 'total'} on:click={() => trafficTab = 'total'}>Total</button>
        </div>
        {@const clientPeak = Math.max(trafficView.client_upload_bytes, trafficView.client_download_bytes, 1)}
        {@const telegramPeak = Math.max(trafficView.telegram_upload_bytes, trafficView.telegram_download_bytes, 1)}
        <div class="traffic-grid">
          <div class="traffic-channel">
            <div class="traffic-channel-heading"><div><strong>Clients</strong><small>S3 and admin connections</small></div><span class="traffic-badge client">API</span></div>
            <div class="traffic-row"><span class="traffic-label"><span class="traffic-icon download" aria-hidden="true">↓</span><span>Download<small>Server → clients</small></span></span><div class="traffic-meter"><span class="download" style={`width:${(trafficView.client_download_bytes / clientPeak) * 100}%`}></span></div><strong>{formatBytes(trafficView.client_download_bytes)}</strong></div>
            <div class="traffic-row"><span class="traffic-label"><span class="traffic-icon upload" aria-hidden="true">↑</span><span>Upload<small>Clients → server</small></span></span><div class="traffic-meter"><span class="upload" style={`width:${(trafficView.client_upload_bytes / clientPeak) * 100}%`}></span></div><strong>{formatBytes(trafficView.client_upload_bytes)}</strong></div>
          </div>
          <div class="traffic-channel">
            <div class="traffic-channel-heading"><div><strong>Telegram server</strong><small>Storage chat payloads</small></div><span class="traffic-badge telegram">TG</span></div>
            <div class="traffic-row"><span class="traffic-label"><span class="traffic-icon download" aria-hidden="true">↓</span><span>Download<small>Telegram → server</small></span></span><div class="traffic-meter"><span class="download" style={`width:${(trafficView.telegram_download_bytes / telegramPeak) * 100}%`}></span></div><strong>{formatBytes(trafficView.telegram_download_bytes)}</strong></div>
            <div class="traffic-row"><span class="traffic-label"><span class="traffic-icon upload" aria-hidden="true">↑</span><span>Upload<small>Server → Telegram</small></span></span><div class="traffic-meter"><span class="upload" style={`width:${(trafficView.telegram_upload_bytes / telegramPeak) * 100}%`}></span></div><strong>{formatBytes(trafficView.telegram_upload_bytes)}</strong></div>
          </div>
        </div>
        <p class="fine-print traffic-note">Payload totals only; protocol overhead is excluded. This session resets on restart; Total is stored in metadata and survives restarts.</p>
      {/if}
    </article>
    <article class="card surface stage-metrics-card" aria-label="Download stage metrics">
      <div class="analytics-heading"><div><p class="card-label">Performance lab</p><h2>Download stage metrics</h2></div><div class="stage-actions"><span class="live-chip"><span aria-hidden="true"></span>Testing view</span><button class="ghost" type="button" on:click={onStageMetricsTest} disabled={stageTestBusy || stageMetrics.test_active_requests > 0}>{stageTestBusy || stageMetrics.test_active_requests > 0 ? 'Testing…' : 'Run test'}</button></div></div>
      {#if loading || !overview}<div class="skeleton" style="height:180px"></div>{:else}
        <div class="stage-summary">
          <div><strong>{formatCount(stageMetrics.active_requests)}</strong><small>active reads</small></div>
          <div><strong>{formatCount(stageMetrics.completed_requests)}</strong><small>completed reads</small></div>
          <div><strong class:bad={stageMetrics.failed_requests > 0}>{formatCount(stageMetrics.failed_requests)}</strong><small>failed reads</small></div>
        </div>
        {#if stageMetrics.last_test}
          <div class="diagnostic-test" aria-label="Last diagnostic stage test">
            <div class="diagnostic-heading"><div><strong>Last diagnostic test</strong><small>One verified chunk from a committed object</small></div><span class:stage-ok={stageMetrics.last_test.status === 'completed'} class:stage-bad={stageMetrics.last_test.status === 'failed'} class="stage-status">{stageMetrics.last_test.status}</span></div>
            <div class="diagnostic-grid"><span>Downloaded <strong>{formatBytes(stageMetrics.last_test.client_bytes)}</strong></span><span>Telegram data <strong>{formatBytes(stageMetrics.last_test.telegram_bytes)}</strong></span><span>First chunk <strong>{formatStageDuration(stageMetrics.last_test.first_chunk_us)}</strong></span><span>Telegram <strong>{formatStageDuration(stageMetrics.last_test.telegram_us)}</strong></span><span>Decrypt <strong>{formatStageDuration(stageMetrics.last_test.decrypt_us)}</strong></span><span>Verify <strong>{formatStageDuration(stageMetrics.last_test.verify_us)}</strong></span><span>Prefetch <strong>{stageMetrics.last_test.prefetch_window_final ?? '—'}/{stageMetrics.last_test.prefetch_window_max ?? '—'}</strong></span><span>Total <strong>{formatStageDuration(stageMetrics.last_test.total_us)}</strong></span></div>
            {#if stageMetrics.last_test.accounts?.length}
              <div class="stage-accounts" aria-label="Diagnostic Telegram account usage"><strong>Telegram accounts</strong>{#each stageMetrics.last_test.accounts as account (account.account_id)}<span><b>{accountLabel(account.account_id)}</b> · {formatBytes(account.telegram_bytes)} · {account.chunks} chunk{account.chunks === 1 ? '' : 's'} · {formatStageDuration(account.telegram_us)}</span>{/each}</div>
            {/if}
          </div>
        {:else}
          <p class="fine-print">Run a dedicated one-chunk diagnostic to measure the server path without borrowing the latest client download.</p>
        {/if}
        {#if stageMetrics.recent.length}
          <div class="stage-table-scroll">
            <table class="stage-table" aria-label="Recent download stage timings">
              <thead><tr><th>Surface</th><th>Status</th><th>Downloaded</th><th>Telegram data</th><th>Accounts</th><th>First chunk</th><th>Telegram</th><th>Retry wait</th><th>Decrypt</th><th>Verify</th><th>Total</th></tr></thead>
              <tbody>
                {#each stageMetrics.recent.slice(0, 8) as sample (sample.request_id)}
                  <tr>
                    <td>{sample.surface}</td>
                    <td><span class:stage-ok={sample.status === 'completed'} class:stage-bad={sample.status === 'failed'} class="stage-status">{sample.status}</span>{#if sample.error}<small>{sample.error}</small>{/if}</td>
                    <td>{formatBytes(sample.client_bytes)}</td>
                    <td>{formatBytes(sample.telegram_bytes)}</td>
                    <td>{#if sample.accounts?.length}<div class="account-pills">{#each sample.accounts as account (account.account_id)}<span title={`${accountLabel(account.account_id)}: ${formatBytes(account.telegram_bytes)}`}>{accountLabel(account.account_id)} · {formatBytes(account.telegram_bytes)}</span>{/each}</div>{:else}—{/if}</td>
                    <td>{formatStageDuration(sample.first_chunk_us)}</td>
                    <td>{formatStageDuration(sample.telegram_us)}</td>
                    <td>{formatStageDuration(sample.retry_wait_us)}</td>
                    <td>{formatStageDuration(sample.decrypt_us)}</td>
                    <td>{formatStageDuration(sample.verify_us)}</td>
                    <td>{formatStageDuration(sample.total_us)}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          <p class="fine-print">Start a public, admin, or S3 download and wait for the five-second Overview refresh. These timings are process-local diagnostics.</p>
        {:else}
          <p class="fine-print">No reads recorded yet. Start a download to capture Telegram, retry, decrypt, verify, and total timings.</p>
        {/if}
      {/if}
    </article>
    <article class="card surface verifier-card">
      <div class="analytics-heading"><div><p class="card-label">Recovery verifier</p><h2>Remote integrity checks</h2></div><span class:healthy={overview?.verifier?.status === 'healthy'} class:attention={overview?.verifier?.status === 'attention'} class:disabled={overview?.verifier?.status === 'disabled'} class="score-chip">{verifierStatusLabel(overview?.verifier?.status ?? 'pending')}</span></div>
      {#if loading || !overview}<div class="skeleton" style="height:132px"></div>{:else}
        <div class="verifier-summary">
          <div><span>Next verifier</span><strong>{formatCountdown(overview.verifier?.next_run_at, overview.verifier?.enabled ?? true)}</strong><small>{overview.verifier?.enabled === false ? 'not scheduled' : `${overview.verifier?.interval_secs ?? 0}s interval`}</small></div>
          <div><span>Random sample</span><strong>{overview.verifier?.chunks_per_object ?? 0} / file</strong><small>fresh chunks per scan</small></div>
          <div><span>Broken files</span><strong class:bad={(overview.verifier?.broken_files ?? 0) > 0}>{formatCount(overview.verifier?.broken_files ?? 0)}</strong><small>confirmed recovery findings</small></div>
          <div><span>Last scan</span><strong>{overview.verifier?.last_scan_duration_ms == null ? '—' : `${overview.verifier.last_scan_duration_ms}ms`}</strong><small>{formatCount(overview.verifier?.scan_runs ?? 0)} run{(overview.verifier?.scan_runs ?? 0) === 1 ? '' : 's'} · {formatCount(overview.verifier?.scan_failures ?? 0)} failed</small></div>
        </div>
        {#if overview.verifier?.problems?.length}
          <div class="verifier-problems" aria-label="Verifier problems">
            <div class="problem-heading"><strong>Problems found</strong><button class="btn-link" type="button" on:click={onRecovery}>Open recovery →</button></div>
            {#each overview.verifier.problems.slice(0, 5) as problem (problem.id)}<div class="problem-row"><span class:confirmed={problem.commit_state === 'recovery_required'} class="problem-dot" aria-hidden="true"></span><div><strong>{problem.path ?? problem.summary}</strong><small>{problem.summary}</small></div><span class="problem-kind">{problem.kind.replaceAll('_', ' ')}</span></div>{/each}
            {#if overview.verifier.problems.length > 5}<p class="fine-print">Showing 5 of {formatCount(overview.verifier.problems.length)} verifier problem(s).</p>{/if}
          </div>
        {:else if overview.verifier?.enabled === false}<p class="fine-print">Automatic verification is disabled. Existing recovery findings remain available, but no new remote checks will run until it is enabled in Telegram settings.</p>{:else}<p class="fine-print">No verifier problems have been found. A temporary Telegram read issue stays retryable and does not mark the file broken.</p>{/if}
      {/if}
    </article>
    <article class="card surface posture-card">
      <div class="analytics-heading"><div><p class="card-label">Operational posture</p><h2>Storage safeguards</h2></div><span class="fine-print">Current snapshot</span></div>
      {#if loading || !overview}<div class="skeleton" style="height:72px"></div>{:else}<div class="posture-grid"><div><span>Recovery markers</span><strong>{formatCount(overview.storage?.recovery_markers ?? 0)}</strong></div><div><span>Recovery-required objects</span><strong>{formatCount(overview.storage?.recovery_required_objects ?? 0)}</strong></div><div><span>Cleanup requiring review</span><strong>{formatCount(transferMetrics.cleanup_recovery_required)}</strong></div><div><span>Configured chunk size</span><strong>{formatBytes(overview.storage?.chunk_size ?? 0)}</strong></div></div>{/if}
    </article>
  </section>
{/if}

<style>
  .overview-head { margin-bottom: .25rem; }
  .overview-head h2 { margin: .25rem 0 0; }
  .analysis-grid { grid-template-columns: 1.25fr .75fr; }
  .insight-grid { grid-template-columns: 1.15fr .85fr; align-items: stretch; }
  .chart-card h2 { margin: .25rem 0 1.2rem; }
  .analytics-card, .checks-card, .posture-card { display: grid; align-content: start; gap: 18px; }
  .analytics-heading { display: flex; align-items: flex-start; justify-content: space-between; gap: 14px; }
  .analytics-heading h2 { margin: .25rem 0 0; }
  .live-chip { display: inline-flex; align-items: center; gap: 7px; flex: 0 0 auto; padding: 6px 9px; border: 1px solid #b8dfce; border-radius: 999px; background: #effaf5; color: #197658; font-size: .7rem; font-weight: 800; }
  .stage-actions { display: flex; align-items: center; gap: 9px; flex-wrap: wrap; justify-content: flex-end; }
  .diagnostic-test { display: grid; gap: 12px; padding: 13px 14px; border: 1px solid #c8dff0; border-radius: 13px; background: linear-gradient(135deg,#f5fbff,#f9fcff); }
  .diagnostic-heading { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
  .diagnostic-heading strong, .diagnostic-heading small { display: block; }
  .diagnostic-heading strong { color: #24526f; font-size: .78rem; }
  .diagnostic-heading small { margin-top: 3px; color: var(--muted); font-size: .7rem; }
  .diagnostic-grid { display: grid; grid-template-columns: repeat(5, minmax(0,1fr)); gap: 8px; }
  .diagnostic-grid span { color: var(--muted); font-size: .68rem; }
  .diagnostic-grid strong { display: block; margin-top: 4px; color: #203b57; font-size: .8rem; }
  .stage-accounts { display: flex; flex-wrap: wrap; align-items: center; gap: 7px; margin-top: 12px; color: var(--muted); font-size: .68rem; }
  .stage-accounts > strong { color: #24526f; }
  .stage-accounts span, .account-pills span { padding: 4px 7px; border: 1px solid #dce7ef; border-radius: 999px; background: #f7fafc; color: #526c82; }
  .account-pills { display: flex; flex-wrap: wrap; gap: 4px; max-width: 220px; white-space: normal; }
  .account-pills span { display: block; font-size: .64rem; }
  .live-chip span { width: 7px; height: 7px; border-radius: 50%; background: #2e9a73; box-shadow: 0 0 0 4px rgba(46,154,115,.12); }
  .score-chip { flex: 0 0 auto; padding: 6px 9px; border: 1px solid #efc88b; border-radius: 999px; background: #fff8e9; color: #9a630f; font-size: .7rem; font-weight: 800; }
  .score-chip.clear, .score-chip.healthy { border-color: #b8dfce; background: #effaf5; color: #197658; }
  .score-chip.disabled { border-color: #d7dfe6; background: #f1f3f5; color: #68798a; }
  .pipeline-chart { display: flex; height: 18px; overflow: hidden; border-radius: 999px; background: #edf1f5; }
  .pipeline-segment { min-width: 0; }
  .pipeline-segment.pending, .pipeline-legend .pending { background: #3d8ac5; }
  .pipeline-segment.failed, .pipeline-legend .failed { background: #c35a5a; }
  .pipeline-segment.cleanup-due, .pipeline-legend .cleanup-due { background: #d59a47; }
  .pipeline-segment.cleanup-scheduled, .pipeline-legend .cleanup-scheduled { background: #8fa8bd; }
  .pipeline-segment.cleanup-recovery, .pipeline-legend .cleanup-recovery { background: #c35a5a; }
  .pipeline-legend { margin-top: -4px; }
  .metric-strip { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 10px; padding-top: 15px; border-top: 1px solid var(--border); }
  .metric-strip div { min-width: 0; }
  .metric-strip strong { display: block; overflow-wrap: anywhere; color: #203b57; font-size: 1rem; }
  .metric-strip small { display: block; margin-top: 3px; color: var(--muted); font-size: .68rem; }
  .check-list { display: grid; gap: 11px; }
  .check-row { display: grid; grid-template-columns: 9px minmax(0, 1fr) auto; align-items: center; gap: 9px; }
  .check-dot { width: 8px; height: 8px; border-radius: 50%; background: #d59a47; box-shadow: 0 0 0 4px rgba(213,154,71,.12); }
  .check-dot.ok { background: #2e9a73; box-shadow: 0 0 0 4px rgba(46,154,115,.12); }
  .check-row strong, .check-row small { display: block; }
  .check-row strong { font-size: .78rem; }
  .check-row small { margin-top: 2px; overflow-wrap: anywhere; color: var(--muted); font-size: .68rem; }
  .check-state { color: #a66a0b; font-size: .65rem; font-weight: 800; text-transform: uppercase; }
  .check-state.ok { color: #197658; }
  .traffic-card { grid-column: 1 / -1; }
  .traffic-tabs { display: inline-flex; gap: 4px; margin: 0 0 16px; padding: 4px; border: 1px solid #dce6ee; border-radius: 10px; background: #f3f7fa; }
  .traffic-tabs button { padding: 7px 12px; border: 0; border-radius: 7px; background: transparent; color: #617891; font-size: .72rem; font-weight: 800; }
  .traffic-tabs button.active { background: #fff; color: #2369a3; box-shadow: 0 1px 3px rgba(32,59,87,.12); }
  .traffic-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px; }
  .traffic-channel { min-width: 0; padding: 15px; border: 1px solid #e1e9f0; border-radius: 14px; background: #fbfcfe; }
  .traffic-channel-heading { display: flex; align-items: flex-start; justify-content: space-between; gap: 10px; margin-bottom: 16px; }
  .traffic-channel-heading strong, .traffic-channel-heading small { display: block; }
  .traffic-channel-heading strong { color: #203b57; font-size: .9rem; }
  .traffic-channel-heading small { margin-top: 3px; color: var(--muted); font-size: .7rem; }
  .traffic-badge { padding: 5px 7px; border-radius: 7px; font-size: .64rem; font-weight: 800; letter-spacing: .06em; }
  .traffic-badge.client { background: #eaf3fb; color: #2779bc; }
  .traffic-badge.telegram { background: #edf8f3; color: #197658; }
  .traffic-row { display: grid; grid-template-columns: 145px minmax(60px, 1fr) auto; align-items: center; gap: 10px; min-width: 0; }
  .traffic-row + .traffic-row { margin-top: 13px; }
  .traffic-label { display: inline-flex; align-items: center; gap: 8px; min-width: 0; color: #334b63; font-size: .75rem; font-weight: 750; }
  .traffic-label > span:last-child { min-width: 0; }
  .traffic-label small { display: block; margin-top: 2px; color: var(--muted); font-size: .63rem; font-weight: 500; white-space: nowrap; }
  .traffic-icon { display: inline-grid; width: 23px; height: 23px; place-items: center; border-radius: 7px; font-size: 1rem; font-weight: 800; }
  .traffic-icon.download { background: #eaf3fb; color: #2779bc; }
  .traffic-icon.upload { background: #fff3df; color: #a66a0b; }
  .traffic-meter { height: 7px; overflow: hidden; border-radius: 999px; background: #e8edf2; }
  .traffic-meter span { display: block; height: 100%; min-width: 2px; border-radius: inherit; transition: width .35s ease; }
  .traffic-meter span.download { background: #3d8ac5; }
  .traffic-meter span.upload { background: #d59a47; }
  .traffic-row > strong { min-width: 60px; color: #203b57; font-size: .76rem; text-align: right; }
  .traffic-note { margin: 0; }
  .stage-metrics-card { grid-column: 1 / -1; }
  .stage-summary { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 12px; }
  .stage-summary > div { padding: 13px 14px; border: 1px solid #e1e9f0; border-radius: 12px; background: #fbfcfe; }
  .stage-summary strong, .stage-summary small { display: block; }
  .stage-summary strong { color: #203b57; font-size: 1.02rem; }
  .stage-summary strong.bad { color: #b24646; }
  .stage-summary small { margin-top: 4px; color: var(--muted); font-size: .68rem; }
  .stage-table-scroll { overflow-x: auto; border: 1px solid #e1e9f0; border-radius: 12px; }
  .stage-table { width: 100%; min-width: 760px; border-collapse: collapse; font-size: .72rem; }
  .stage-table th, .stage-table td { padding: 10px 9px; border-bottom: 1px solid #edf1f5; text-align: left; white-space: nowrap; }
  .stage-table th { color: var(--muted); font-size: .64rem; letter-spacing: .06em; text-transform: uppercase; }
  .stage-table tbody tr:last-child td { border-bottom: 0; }
  .stage-status { padding: 4px 7px; border-radius: 999px; background: #f1f3f5; color: #68798a; font-size: .64rem; font-weight: 800; text-transform: capitalize; }
  .stage-status.stage-ok { background: #effaf5; color: #197658; }
  .stage-status.stage-bad { background: #fff0f0; color: #b24646; }
  .stage-table td small { display: block; max-width: 180px; overflow: hidden; color: #b24646; font-size: .64rem; text-overflow: ellipsis; }
  .posture-card { grid-column: 1 / -1; }
  .verifier-card { grid-column: 1 / -1; }
  .score-chip.attention { border-color: #efc88b; background: #fff8e9; color: #9a630f; }
  .verifier-summary { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; }
  .verifier-summary > div { padding: 13px 14px; border: 1px solid #e1e9f0; border-radius: 12px; background: #fbfcfe; }
  .verifier-summary span, .verifier-summary strong, .verifier-summary small { display: block; }
  .verifier-summary span { color: var(--muted); font-size: .7rem; }
  .verifier-summary strong { margin-top: 5px; color: #203b57; font-size: 1.02rem; }
  .verifier-summary strong.bad { color: #b24646; }
  .verifier-summary small { margin-top: 4px; color: var(--muted); font-size: .68rem; }
  .verifier-problems { display: grid; gap: 9px; padding-top: 15px; border-top: 1px solid var(--border); }
  .problem-heading { display: flex; align-items: center; justify-content: space-between; gap: 10px; color: #203b57; font-size: .82rem; }
  .problem-row { display: grid; grid-template-columns: 9px minmax(0, 1fr) auto; align-items: center; gap: 9px; padding: 9px 0; border-bottom: 1px solid #edf1f5; }
  .problem-row:last-of-type { border-bottom: 0; }
  .problem-dot { width: 8px; height: 8px; border-radius: 50%; background: #d59a47; box-shadow: 0 0 0 4px rgba(213,154,71,.12); }
  .problem-dot.confirmed { background: #c35a5a; box-shadow: 0 0 0 4px rgba(195,90,90,.12); }
  .problem-row strong, .problem-row small { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .problem-row strong { color: #294967; font-size: .76rem; }
  .problem-row small { margin-top: 3px; color: var(--muted); font-size: .67rem; }
  .problem-kind { color: #7890a6; font-size: .63rem; text-transform: uppercase; }
  .posture-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; }
  .posture-grid div { padding: 12px 13px; border: 1px solid #e1e9f0; border-radius: 12px; background: #fbfcfe; }
  .posture-grid span, .posture-grid strong { display: block; }
  .posture-grid span { color: var(--muted); font-size: .7rem; line-height: 1.35; }
  .posture-grid strong { margin-top: 6px; color: #203b57; font-size: 1.02rem; }
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
  @media (max-width: 760px) { .analysis-grid, .insight-grid, .traffic-grid { grid-template-columns: 1fr; } .posture-card, .traffic-card, .verifier-card, .stage-metrics-card { grid-column: auto; } .posture-grid, .verifier-summary, .stage-summary { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
  @media (max-width: 480px) { .metric-strip { grid-template-columns: 1fr; gap: 8px; } .posture-grid, .verifier-summary, .stage-summary { grid-template-columns: 1fr; } .traffic-row { grid-template-columns: minmax(0, 1fr) auto; gap: 8px; } .traffic-label { grid-column: 1 / -1; } .traffic-meter { grid-column: 1; } }
</style>
