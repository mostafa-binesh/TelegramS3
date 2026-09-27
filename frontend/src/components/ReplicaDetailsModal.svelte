<script lang="ts">
  import {listAccounts, queueReplication} from '../lib/api';
  import type {AccountInfo, ReplicaInfo} from '../lib/types';

  export let open = false;
  export let title = '';
  export let items: ReplicaInfo[] = [];
  export let loading = false;
  export let error = '';
  export let csrf: string | null | undefined;
  export let bucket = '';
  export let scopeKeys: string[] = [];
  export let scopeLabel = 'This bucket';

  let accounts: AccountInfo[] = [];
  let accountsLoading = false;
  let accountError = '';
  let sourceId = '';
  let targetId = '';
  let mode: 'one_time' | 'automatic' = 'one_time';
  let accessMode: 'replica' | 'access' = 'replica';
  let queueBusy = false;
  let queueMessage = '';
  let queueError = '';
  let loadedForOpen = false;

  $: if (open && !loadedForOpen) {
    loadedForOpen = true;
    void refreshAccounts();
  }
  $: if (!open) loadedForOpen = false;
  $: if (!sourceId && accounts[0]) sourceId = accounts[0].id;
  $: if (!targetId && accounts[1]) targetId = accounts[1].id;
  $: grouped = accounts.map((account) => ({
    account,
    copies: items.filter((item) => item.account_id === account.id && item.mode === 'replica').length,
    access: items.filter((item) => item.account_id === account.id && item.mode === 'access').length
  })).filter((entry) => entry.copies || entry.access);

  async function refreshAccounts() {
    accountsLoading = true;
    accountError = '';
    try { accounts = (await listAccounts(csrf)).accounts; }
    catch (cause) { accountError = cause instanceof Error ? cause.message : 'Unable to load account details'; }
    finally { accountsLoading = false; }
  }

  async function replicate() {
    if (!sourceId || !targetId || sourceId === targetId || !bucket) return;
    queueBusy = true; queueMessage = ''; queueError = '';
    try {
      await queueReplication(csrf, {source_account_id: sourceId, target_account_id: targetId, bucket, keys: scopeKeys, mode, access_mode: accessMode});
      queueMessage = scopeKeys.length ? `Replication queued for ${scopeKeys.length} selected object(s).` : 'Bucket replication queued.';
    } catch (cause) { queueError = cause instanceof Error ? cause.message : 'Unable to queue replication'; }
    finally { queueBusy = false; }
  }
</script>

{#if open}
  <div class="backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (open = false)}>
    <div class="modal" role="dialog" aria-modal="true" aria-labelledby="replica-title">
      <div class="head"><div><p class="card-label">Account copies and access</p><h2 id="replica-title">{title}</h2><p class="scope-note">Scope: <strong>{scopeLabel}</strong></p></div><button class="icon-button" aria-label="Close account access details" on:click={() => open = false}>×</button></div>
      {#if loading}<div class="skeleton-stack"><div class="skeleton" style="height:46px"></div><div class="skeleton" style="height:46px"></div></div>{:else if error}<p class="error">{error}</p>{:else}
        <section class="detail-section"><div class="section-heading"><div><p class="card-label">Availability</p><h3>Who can serve these files</h3></div><span class="count-pill">{items.length} chunk record{items.length === 1 ? '' : 's'}</span></div>
          {#if !items.length}<p class="empty">No replica or access records have been registered for this location yet. The primary account remains the source.</p>{:else}<div class="account-summary">{#each grouped as entry (entry.account.id)}<div class="account-summary-row"><div class="account-avatar">{entry.account.label.slice(0,1).toUpperCase()}</div><div><strong>{entry.account.label}</strong><small>{entry.account.phone || 'Phone hidden'} · {entry.account.storage_chat_id || 'chat not listed'}</small></div><span>{entry.copies ? `${entry.copies} copy${entry.copies === 1 ? '' : 'ies'}` : ''}{entry.copies && entry.access ? ' · ' : ''}{entry.access ? `${entry.access} access` : ''}</span></div>{/each}</div>{/if}
          {#if items.length}<div class="list">{#each items as item (item.object_id + ':' + item.chunk_order + ':' + item.account_id + ':' + item.mode)}<div class="row"><div><strong>{item.account_label}</strong><small>{item.mode === 'access' ? 'Access-only via shared chat' : 'Physical replica'} · chunk {item.chunk_order + 1}</small></div><span class:access={item.mode === 'access'}>{item.state}</span></div>{/each}</div>{/if}
          <p class="fine-print">Round-robin reads use the primary location and ready physical replicas. Access-only records require that account to see the source group.</p>
        </section>
        <section class="detail-section replication-editor"><div class="section-heading"><div><p class="card-label">Replication controls</p><h3>Copy this scope to another account</h3></div></div>
          {#if accountsLoading}<p class="fine-print">Loading account options…</p>{:else if accountError}<p class="error">{accountError}</p>{:else if accounts.length < 2}<p class="empty">Add a second connection in Accounts before creating a replica or access grant.</p>{:else}<div class="form-grid"><label>Source account<select bind:value={sourceId}>{#each accounts as account}<option value={account.id}>{account.label}</option>{/each}</select></label><label>Target account<select bind:value={targetId}>{#each accounts as account}<option value={account.id}>{account.label}</option>{/each}</select></label><label>Schedule<select bind:value={mode}><option value="one_time">One-time replicate</option><option value="automatic">Automatic replication</option></select></label><label>Target mode<select bind:value={accessMode}><option value="replica">Physical replica</option><option value="access">Access-only via shared chat</option></select></label></div><p class="fine-print">The selected scope is persisted in the worker job. Automatic jobs repeat every five minutes after their last run.</p><button class="primary" type="button" on:click={replicate} disabled={queueBusy || !sourceId || !targetId || sourceId === targetId}>{queueBusy ? 'Queueing…' : scopeKeys.length ? 'Queue selected replication' : 'Queue bucket replication'}</button>{/if}
          {#if queueMessage}<p class="notice" role="status">{queueMessage}</p>{/if}{#if queueError}<p class="error" role="alert">{queueError}</p>{/if}
        </section>
      {/if}
    </div>
  </div>
{/if}

<style>
  .backdrop{position:fixed;inset:0;z-index:30;display:grid;place-items:center;padding:20px;background:rgba(12,25,42,.58)}.modal{width:min(720px,100%);max-height:calc(100vh - 40px);overflow:auto;padding:22px;border:1px solid var(--border);border-radius:var(--radius-lg);background:var(--surface);box-shadow:0 20px 60px rgba(13,31,52,.25)}.head{display:flex;justify-content:space-between;gap:16px;align-items:flex-start}.head h2{margin:.25rem 0 .35rem;color:#203b57;overflow-wrap:anywhere}.scope-note{margin:0;color:var(--muted);font-size:.76rem}.icon-button{width:36px;height:36px;padding:0;border-radius:50%;background:var(--accent-soft);color:var(--text);font-size:1.35rem}.detail-section{display:grid;gap:12px;margin-top:18px;padding-top:18px;border-top:1px solid #edf1f5}.section-heading{display:flex;align-items:flex-start;justify-content:space-between;gap:14px}.section-heading h3{margin:.25rem 0;color:#203b57}.count-pill{padding:6px 9px;border-radius:999px;background:#eaf3ff;color:#28679d;font-size:.7rem;font-weight:800;white-space:nowrap}.account-summary,.list{display:grid;gap:8px}.account-summary-row,.row{display:flex;align-items:center;justify-content:space-between;gap:12px;padding:11px 12px;border:1px solid #e0eaf1;border-radius:12px;background:#fbfdff}.account-summary-row>div:nth-child(2),.row div{display:grid;gap:3px;min-width:0;flex:1}.account-avatar{display:grid;place-items:center;flex:0 0 auto;width:34px;height:34px;border-radius:10px;background:#dceefe;color:#216c9e;font-weight:850}.account-summary-row span,.row span{padding:5px 8px;border-radius:999px;background:#e3f5e9;color:#17604a;font-size:.68rem;font-weight:800;white-space:nowrap}.row span.access{background:#eaf0fb;color:#3b5a8e}.account-summary-row small,.row small{color:var(--muted);font-size:.7rem;overflow-wrap:anywhere}.empty,.error{padding:10px 0;color:var(--muted)}.error{color:var(--danger)}.form-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}.form-grid label{display:grid;gap:6px;color:#426079;font-size:.75rem;font-weight:800}.form-grid select{width:100%;box-sizing:border-box;padding:10px 12px;border:1px solid var(--border);border-radius:10px;background:#fff;color:var(--ink);font:inherit}.notice{padding:11px 13px;border:1px solid #bde4d2;border-radius:12px;background:#f1fcf6;color:#17604a;font-size:.78rem}@media(max-width:620px){.form-grid{grid-template-columns:1fr}.account-summary-row,.row{align-items:flex-start;flex-wrap:wrap}.account-summary-row>span,.row>span{margin-left:auto}}
</style>
