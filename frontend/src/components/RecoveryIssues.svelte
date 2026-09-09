<script lang="ts">
  import { acknowledgeRecovery, unacknowledgeRecovery } from '../lib/api';
  import { formatCount, formatTimestamp, normalizeError } from '../lib/format';
  import type { RecoveryIssue, RecoveryState } from '../lib/types';

  export let recovery: RecoveryState | null | undefined = null;
  export let csrf: string | null | undefined;
  export let loading = false;
  /** Re-reads the overview so this list and the Overview count cannot disagree. */
  export let onChanged: () => Promise<void> = async () => {};
  export let onRefresh: () => void = () => {};

  type Override = { acknowledged_at: string; acknowledged_by: string } | null;

  /** Optimistic patches keyed by issue id, cleared once fresh data lands. */
  let overrides: Record<string, Override> = {};
  let pending: Record<string, boolean> = {};
  let actionError = '';

  function acknowledgedState(issue: RecoveryIssue) {
    if (issue.id in overrides) return overrides[issue.id];
    return issue.acknowledged_at
      ? { acknowledged_at: issue.acknowledged_at, acknowledged_by: issue.acknowledged_by ?? '' }
      : null;
  }

  function label(issue: RecoveryIssue) {
    if (issue.path) return issue.path;
    if (issue.bucket && issue.key) return `${issue.bucket}/${issue.key}`;
    if (issue.bucket) return issue.bucket;
    return issue.kind;
  }

  async function setAcknowledged(issue: RecoveryIssue, next: boolean) {
    actionError = '';
    pending = { ...pending, [issue.id]: true };
    overrides = {
      ...overrides,
      [issue.id]: next
        ? { acknowledged_at: new Date().toISOString(), acknowledged_by: 'you' }
        : null
    };
    try {
      if (next) await acknowledgeRecovery(csrf, [issue.id]);
      else await unacknowledgeRecovery(csrf, [issue.id]);
      await onChanged();
    } catch (cause) {
      actionError = normalizeError(cause);
    } finally {
      // Either fresh data or the rollback supersedes the optimistic patch.
      const { [issue.id]: _dropped, ...rest } = overrides;
      overrides = rest;
      const { [issue.id]: _done, ...restPending } = pending;
      pending = restPending;
    }
  }

  $: issues = recovery?.issues ?? [];
  $: open = issues.filter((issue) => acknowledgedState(issue) === null);
  $: acknowledged = issues.filter((issue) => acknowledgedState(issue) !== null);
</script>

<section class="card surface">
  <div class="section-head">
    <div>
      <p class="card-label">Corrupted and missing files</p>
      <h2>
        {formatCount(open.length)}
        {open.length === 1 ? 'file needs attention' : 'files need attention'}
      </h2>
      <p class="fine-print">Expand an issue for details. Acknowledging it keeps it here but removes it from the Overview count.</p>
      {#if recovery?.checked_at}
        <p class="fine-print">Snapshot refreshed {formatTimestamp(recovery.checked_at)}.</p>
      {/if}
    </div>
    <button class="ghost" type="button" on:click={onRefresh} disabled={loading}>
      {#if loading}<span class="spinner" aria-hidden="true"></span>{/if}
      Refresh
    </button>
  </div>

  {#if actionError}
    <p role="alert" class="error-hint">{actionError}</p>
  {/if}

  {#if recovery?.scan_error}
    <p role="alert" class="error-hint">Recovery scan unavailable: {recovery.scan_error}</p>
  {:else if loading && !recovery}
    <div class="skeleton-stack">
      <div class="skeleton" style="height:52px"></div>
      <div class="skeleton" style="height:52px"></div>
    </div>
  {:else if issues.length === 0}
    <p class="empty-state">
      <span class="empty-mark" aria-hidden="true">✓</span>
      No missing or corrupted files were detected.
    </p>
  {:else}
    {#if open.length === 0}
      <p class="empty-state">
        <span class="empty-mark" aria-hidden="true">✓</span>
        Every detected issue has been acknowledged.
      </p>
    {/if}
    <div class="recovery-list">
      {#each open as issue (issue.id)}
        <details class="recovery-item">
          <summary>
            <span>{label(issue)}</span>
            <small>{issue.summary}</small>
          </summary>
          <div class="recovery-meta">
            <span>{issue.kind}</span>
            {#if issue.commit_state}<span>{issue.commit_state}</span>{/if}
            {#if issue.object_id}<span>{issue.object_id}</span>{/if}
          </div>
          <ul>
            {#each issue.details as detail}
              <li>{detail}</li>
            {/each}
          </ul>
          <div class="issue-actions">
            <button
              class="ghost"
              type="button"
              disabled={pending[issue.id]}
              on:click={() => setAcknowledged(issue, true)}
            >
              Acknowledge
            </button>
          </div>
        </details>
      {/each}
    </div>

    {#if acknowledged.length > 0}
      <details class="acknowledged-group">
        <summary>Acknowledged ({formatCount(acknowledged.length)})</summary>
        <div class="recovery-list">
          {#each acknowledged as issue (issue.id)}
            {@const ack = acknowledgedState(issue)}
            <details class="recovery-item is-acknowledged">
              <summary>
                <span>{label(issue)}</span>
                <small>{issue.summary}</small>
              </summary>
              <p class="fine-print ack-line">
                Acknowledged {formatTimestamp(ack?.acknowledged_at)}
                {#if ack?.acknowledged_by}by {ack.acknowledged_by}{/if}
              </p>
              <div class="recovery-meta">
                <span>{issue.kind}</span>
                {#if issue.commit_state}<span>{issue.commit_state}</span>{/if}
                {#if issue.object_id}<span>{issue.object_id}</span>{/if}
              </div>
              <ul>
                {#each issue.details as detail}
                  <li>{detail}</li>
                {/each}
              </ul>
              <div class="issue-actions">
                <button
                  class="ghost"
                  type="button"
                  disabled={pending[issue.id]}
                  on:click={() => setAcknowledged(issue, false)}
                >
                  Restore to list
                </button>
              </div>
            </details>
          {/each}
        </div>
      </details>
    {/if}
  {/if}
</section>

<style>
  .recovery-list {
    display: grid;
    gap: 0.6rem;
    margin-top: 1rem;
  }
  .recovery-item {
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    padding: 0.85rem 1rem;
    background: var(--surface);
  }
  .recovery-item[open] {
    border-color: color-mix(in srgb, var(--accent) 35%, var(--border));
  }
  .recovery-item.is-acknowledged {
    opacity: 0.72;
  }
  .recovery-item summary {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem 1rem;
    align-items: baseline;
    cursor: pointer;
    list-style: none;
  }
  .recovery-item summary::-webkit-details-marker {
    display: none;
  }
  .recovery-item summary::before {
    content: '▸';
    color: var(--muted);
    transition: transform 120ms ease;
  }
  .recovery-item[open] summary::before {
    transform: rotate(90deg);
  }
  .recovery-item summary span {
    font-weight: 700;
    overflow-wrap: anywhere;
  }
  .recovery-item summary small {
    color: var(--muted);
  }
  .recovery-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin: 0.55rem 0 0.35rem;
    font-size: 0.82rem;
  }
  .recovery-meta span {
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    background: color-mix(in srgb, var(--text) 7%, transparent);
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  .recovery-item ul {
    margin: 0.4rem 0 0;
    padding-left: 1.2rem;
    color: var(--muted);
  }
  .ack-line {
    margin: 0.5rem 0 0;
  }
  .issue-actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 0.75rem;
  }
  .issue-actions button {
    padding: 0.45rem 0.9rem;
    font-size: 0.88rem;
  }
  .acknowledged-group {
    margin-top: 1.1rem;
    padding-top: 0.9rem;
    border-top: 1px solid var(--border);
  }
  .acknowledged-group > summary {
    cursor: pointer;
    color: var(--muted);
    font-size: 0.9rem;
  }
</style>
