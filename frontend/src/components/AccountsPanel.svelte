<script lang="ts">
  import {onMount} from 'svelte';
  import {deleteAccount, listAccounts, listBuckets, listReplicationJobs, listRechunkJobs, queueReplication, saveAccount} from '../lib/api';
  import {formatBytes} from '../lib/format';
  import type {AccountInfo, ReplicationJob, RechunkJob} from '../lib/types';
  import LoadError from './LoadError.svelte';

  export let csrf: string | null | undefined;
  export let buckets: {name: string}[] = [];
  let availableBuckets: {name: string}[] = buckets;
  let accounts: AccountInfo[] = [];
  let replication: ReplicationJob[] = [];
  let rechunk: RechunkJob[] = [];
  let loading = true;
  let busy = false;
  let error = '';
  let message = '';
  let label = '';
  let phone = '';
  let apiId = '';
  let apiHash = '';
  let sessionPath = '';
  let storageChatId = '';
  let proxyUrl = '';
  let sourceId = '';
  let targetId = '';
  let bucket = '';
  let mode: 'one_time'|'automatic' = 'one_time';
  let accessMode: 'replica'|'access' = 'replica';

  async function refresh() {
    loading = true;
    try {
      const [a, r, c, b] = await Promise.all([listAccounts(csrf), listReplicationJobs(csrf), listRechunkJobs(csrf), listBuckets(csrf, {page: 1, pageSize: 100})]);
      accounts = a.accounts; replication = r.jobs; rechunk = c.jobs;
      availableBuckets = b.buckets;
      if (!sourceId && accounts[0]) sourceId = accounts[0].id;
      if (!targetId && accounts[1]) targetId = accounts[1].id;
      error = '';
    } catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to load account state'; }
    finally { loading = false; }
  }

  async function addAccount() {
    busy = true; message = ''; error = '';
    try {
      await saveAccount(csrf, {label, phone, telegram_api_id: apiId, telegram_api_hash: apiHash,
        telegram_session_path: sessionPath || undefined, telegram_storage_chat_id: storageChatId,
        telegram_proxy_url: proxyUrl || undefined, telegram_proxy_mode: 'auto'});
      label = ''; phone = ''; apiId = ''; apiHash = ''; sessionPath = ''; storageChatId = ''; proxyUrl = '';
      message = 'Connection saved. Use a separate session file when adding the same Telegram account again.';
      await refresh();
    } catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to save connection'; }
    finally { busy = false; }
  }

  async function removeAccount(account: AccountInfo) {
    if (!confirm(`Remove ${account.label}? Existing files are not deleted.`)) return;
    busy = true;
    try { await deleteAccount(csrf, account.id); await refresh(); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to remove connection'; }
    finally { busy = false; }
  }

  async function createReplication() {
    busy = true; message = ''; error = '';
    try { await queueReplication(csrf, {source_account_id: sourceId, target_account_id: targetId, bucket, mode, access_mode: accessMode}); message = 'Replication job queued. Progress is durable and survives a restart.'; await refresh(); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to queue replication'; }
    finally { busy = false; }
  }

  onMount(() => { void refresh(); let timer: ReturnType<typeof setTimeout>; let stopped = false; const poll = async () => { if (!document.hidden) await refresh(); if (!stopped) timer = setTimeout(poll, 5000); }; timer = setTimeout(poll, 5000); return () => { stopped = true; clearTimeout(timer); }; });
</script>

<section class="page-grid">
  <div class="card surface hero"><div><p class="card-label">Accounts and connections</p><h2>Telegram account pool</h2><p class="fine-print">Connections are independent transports. The same Telegram account is supported when each connection uses its own session file.</p></div><span class="count-pill">{accounts.length} connections</span></div>
  {#if error}<LoadError title="Account service unavailable" message={error} onRetry={refresh}/>{/if}
  {#if message}<div class="notice" role="status">{message}</div>{/if}
  <div class="card surface"><div class="section-head"><div><p class="card-label">Connection registry</p><h3>Configured connections</h3></div><button class="ghost" on:click={refresh} disabled={loading}>Refresh</button></div>
    {#if loading}<div class="skeleton-stack"><div class="skeleton" style="height:48px"></div><div class="skeleton" style="height:48px"></div></div>{:else if !accounts.length}<p class="empty">No additional connections are configured yet.</p>{:else}<div class="account-list">{#each accounts as account (account.id)}<div class="account-row"><div class="account-icon">{account.label.slice(0,1).toUpperCase()}</div><div class="account-copy"><strong>{account.label}</strong><small>{account.phone || 'Phone hidden'} · chat {account.storage_chat_id || 'not set'}</small><small>{account.replica_objects} replica objects · {account.access_objects} access-only objects</small></div><span class="state">{account.state}</span><button class="ghost danger" on:click={() => removeAccount(account)} disabled={busy}>Remove</button></div>{/each}</div>{/if}
  </div>

  <div class="card surface"><div class="section-head"><div><p class="card-label">Add connection</p><h3>Register another Telegram transport</h3></div></div><div class="form-grid"><label>Label<input bind:value={label} placeholder="Backup account"/></label><label>Phone (optional)<input bind:value={phone} placeholder="+989…"/></label><label>API ID<input bind:value={apiId} inputmode="numeric"/></label><label>API hash<input bind:value={apiHash} type="password"/></label><label>Session file<input bind:value={sessionPath} placeholder="data/telegram-account-2.session"/></label><label>Storage chat ID<input bind:value={storageChatId} placeholder="-100…"/></label><label class="wide">SOCKS5 / proxy URL (optional)<input bind:value={proxyUrl} placeholder="socks5://127.0.0.1:12334"/></label></div><p class="fine-print">This registers an existing authorized session. Login verification remains in the Telegram connection wizard; never reuse one session file concurrently.</p><button class="primary" on:click={addAccount} disabled={busy || !label || !apiId || !apiHash || !storageChatId}>{busy ? 'Saving…' : 'Save connection'}</button></div>

  <div class="card surface"><div class="section-head"><div><p class="card-label">Replication</p><h3>Copy or grant bucket access</h3></div></div><div class="form-grid"><label>Source account<select bind:value={sourceId}>{#each accounts as account}<option value={account.id}>{account.label}</option>{/each}</select></label><label>Target account<select bind:value={targetId}>{#each accounts as account}<option value={account.id}>{account.label}</option>{/each}</select></label><label>Bucket<select bind:value={bucket}><option value="">Choose a bucket</option>{#each availableBuckets as item}<option value={item.name}>{item.name}</option>{/each}</select></label><label>Schedule<select bind:value={mode}><option value="one_time">One-time replicate</option><option value="automatic">Automatic replication</option></select></label><label>Target mode<select bind:value={accessMode}><option value="replica">Physical replica</option><option value="access">Access-only via shared chat</option></select></label></div><p class="fine-print">Physical replicas upload durable copies to the target account. Access-only entries retain the source location and require the target session to see that group/chat.</p><button class="primary" on:click={createReplication} disabled={busy || !sourceId || !targetId || sourceId === targetId || !bucket}>{busy ? 'Queueing…' : 'Queue replication'}</button></div>

  <div class="card surface"><div class="section-head"><div><p class="card-label">Worker queues</p><h3>Replication and re-chunk progress</h3></div></div>
    {#if !replication.length && !rechunk.length}
      <p class="empty">No account jobs yet. Bulk re-chunking is started from Buckets.</p>
    {:else}
      <div class="job-list">
        {#each replication as job}
          <div class="job-row"><div><strong>{job.bucket}</strong><small>{job.mode} · {job.access_mode} · {job.state}</small></div><div class="job-progress"><progress max={Math.max(job.chunks_total, 1)} value={job.chunks_done}></progress><small>{job.chunks_done}/{job.chunks_total} chunks · {formatBytes(job.bytes_done)}</small></div></div>
        {/each}
        {#each rechunk as job}
          <div class="job-row"><div><strong>{job.key}</strong><small>re-chunk to {formatBytes(job.new_chunk_size)} · {job.state}</small></div><div class="job-progress"><progress max={Math.max(job.chunks_total, 1)} value={job.chunks_done}></progress><small>{job.chunks_done}/{job.chunks_total} chunks · {formatBytes(job.bytes_done)}</small></div></div>
        {/each}
      </div>
    {/if}
  </div>
</section>

<style>
  .page-grid{display:grid;gap:18px}.hero{display:flex;justify-content:space-between;gap:20px;align-items:flex-start}.hero h2,.card h3{margin:.25rem 0;color:#203b57}.count-pill,.state{display:inline-flex;align-items:center;padding:7px 11px;border-radius:999px;background:#e5f5f0;color:#17604a;font-size:.75rem;font-weight:800;white-space:nowrap}.notice{padding:12px 16px;border:1px solid #bde4d2;border-radius:12px;background:#f1fcf6;color:#17604a}.account-list,.job-list{display:grid;gap:9px}.account-row,.job-row{display:flex;align-items:center;gap:13px;padding:13px;border:1px solid #e0eaf1;border-radius:13px;background:#fbfdff}.account-icon{display:grid;place-items:center;flex:0 0 auto;width:38px;height:38px;border-radius:12px;background:#dceefe;color:#216c9e;font-weight:850}.account-copy{display:grid;gap:3px;min-width:0;flex:1}.account-copy strong,.job-row strong{color:#203b57}.account-copy small,.job-row small{color:var(--muted);font-size:.72rem;overflow-wrap:anywhere}.danger{color:#a63333}.form-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:13px;margin-bottom:14px}.form-grid label{display:grid;gap:6px;color:#426079;font-size:.75rem;font-weight:800}.form-grid input,.form-grid select{width:100%;box-sizing:border-box;padding:10px 12px;border:1px solid var(--border);border-radius:10px;background:#fff;color:var(--ink);font:inherit;font-weight:500}.form-grid .wide{grid-column:1/-1}.job-row{justify-content:space-between}.job-progress{display:grid;gap:4px;min-width:180px}.job-progress progress{width:100%;accent-color:var(--accent)}.empty{padding:18px 0;color:var(--muted)}@media(max-width:700px){.hero,.account-row,.job-row{align-items:stretch;flex-direction:column}.form-grid{grid-template-columns:1fr}.form-grid .wide{grid-column:auto}.job-progress{min-width:0}}
</style>
