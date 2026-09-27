<script lang="ts">
  import {onMount} from 'svelte';
  import {deleteAccount, listAccounts, listBuckets, listReplicationJobs, listRechunkJobs, queueReplication, saveAccount} from '../lib/api';
  import {formatBytes} from '../lib/format';
  import type {AccountInfo, ReplicationJob, RechunkJob} from '../lib/types';
  import type {AccountsTab} from '../lib/router';
  import LoadError from './LoadError.svelte';
  import TelegramPanel from './TelegramPanel.svelte';

  export let csrf: string | null | undefined;
  export let buckets: {name: string}[] = [];
  export let accountsTab: AccountsTab = 'connections';
  export let onTabChange: (tab: AccountsTab) => void = () => {};

  // The primary connection is hosted here with the account registry. Storage
  // policy remains on the separate Telegram settings page.
  export let overview: any = null;
  export let session: any = null;
  export let telegramApiId = '';
  export let telegramApiHash = '';
  export let telegramStorageChatId = '';
  export let telegramProxyUrl = '';
  export let telegramProxyUsername = '';
  export let telegramProxyPassword = '';
  export let telegramProxyMode = 'auto';
  export let telegramAccountPhone = '';
  export let settingsBusy = false;
  export let settingsError = '';
  export let settingsMessage = '';
  export let showWizard = false;
  export let wizardComponent: any = null;
  export let onSave: () => Promise<void> | void = () => {};
  export let onManageOperators: () => void = () => {};
  export let onToggleWizard: (open: boolean) => void = () => {};
  export let onWizardDone: () => void = () => {};
  export let onWizardClose: () => void = () => {};
  export let onRemoveConnection: (phoneConfirmation: string, deleteUploadedFiles: boolean) => Promise<void> = async () => {};

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

  $: selectedBucket = bucket || availableBuckets[0]?.name || '';
  $: accountCountLabel = `${accounts.length} account${accounts.length === 1 ? '' : 's'}`;

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
    try {
      await queueReplication(csrf, {source_account_id: sourceId, target_account_id: targetId, bucket: selectedBucket, mode, access_mode: accessMode});
      message = 'Replication job queued. Progress is durable and survives a restart.';
      await refresh();
    } catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to queue replication'; }
    finally { busy = false; }
  }

  onMount(() => {
    void refresh();
    let timer: ReturnType<typeof setTimeout>; let stopped = false;
    const poll = async () => { if (!document.hidden) await refresh(); if (!stopped) timer = setTimeout(poll, 5000); };
    timer = setTimeout(poll, 5000);
    return () => { stopped = true; clearTimeout(timer); };
  });
</script>

<section class="page-grid">
  <div class="card surface hero"><div><p class="card-label">Accounts and connections</p><h2>Telegram account pool</h2><p class="fine-print">Primary and additional Telegram transports live together here. The same Telegram account is supported when each connection uses its own session file.</p></div><span class="count-pill">{accountCountLabel}</span></div>
  <div class="account-tabs" role="tablist" aria-label="Account management sections">
    <button class:active={accountsTab === 'connections'} type="button" role="tab" aria-selected={accountsTab === 'connections'} on:click={() => onTabChange('connections')}><span class="tab-icon">⌁</span><span><strong>Connections</strong><small>Accounts and health</small></span></button>
    <button class:active={accountsTab === 'replication'} type="button" role="tab" aria-selected={accountsTab === 'replication'} on:click={() => onTabChange('replication')}><span class="tab-icon green">⇄</span><span><strong>Replication</strong><small>Copies and access</small></span></button>
    <button class:active={accountsTab === 'maintenance'} type="button" role="tab" aria-selected={accountsTab === 'maintenance'} on:click={() => onTabChange('maintenance')}><span class="tab-icon amber">◈</span><span><strong>Maintenance</strong><small>Worker progress</small></span></button>
  </div>
  {#if error}<LoadError title="Account service unavailable" message={error} onRetry={refresh}/>{/if}
  {#if message}<div class="notice" role="status">{message}</div>{/if}

  {#if accountsTab === 'connections'}
    <TelegramPanel bind:telegramApiId bind:telegramApiHash bind:telegramStorageChatId bind:telegramProxyUrl bind:telegramProxyUsername bind:telegramProxyPassword bind:telegramProxyMode bind:telegramAccountPhone {overview} {session} telegramTab="connection" hideTabs {settingsBusy} {settingsError} {settingsMessage} {showWizard} {wizardComponent} {onSave} {onManageOperators} {onToggleWizard} {onWizardDone} {onWizardClose} {onRemoveConnection}/>
    <div class="card surface"><div class="section-head"><div><p class="card-label">Connection registry</p><h3>All configured accounts</h3></div><button class="ghost" on:click={refresh} disabled={loading}>Refresh</button></div>
      {#if loading}<div class="skeleton-stack"><div class="skeleton" style="height:48px"></div><div class="skeleton" style="height:48px"></div></div>{:else if !accounts.length}<p class="empty">No Telegram accounts are configured yet.</p>{:else}<div class="account-list">{#each accounts as account, index (account.id)}<div class="account-row"><div class="account-icon">{index === 0 ? 'P' : account.label.slice(0,1).toUpperCase()}</div><div class="account-copy"><strong>{account.label}{#if index === 0}<span class="primary-tag">Primary</span>{/if}</strong><small>{account.phone || 'Phone hidden'} · chat {account.storage_chat_id || 'not set'}</small><small>{account.replica_objects} replica objects · {account.access_objects} access-only objects</small></div><span class="state">{account.state}</span>{#if index > 0}<button class="ghost danger" on:click={() => removeAccount(account)} disabled={busy}>Remove</button>{/if}</div>{/each}</div>{/if}
    </div>
    <div class="card surface"><div class="section-head"><div><p class="card-label">Add connection</p><h3>Register another Telegram transport</h3></div></div><div class="form-grid"><label>Label<input bind:value={label} placeholder="Backup account"/></label><label>Phone (optional)<input bind:value={phone} placeholder="+989…"/></label><label>API ID<input bind:value={apiId} inputmode="numeric"/></label><label>API hash<input bind:value={apiHash} type="password"/></label><label>Session file<input bind:value={sessionPath} placeholder="data/telegram-account-2.session"/></label><label>Storage chat ID<input bind:value={storageChatId} placeholder="-100…"/></label><label class="wide">SOCKS5 / proxy URL (optional)<input bind:value={proxyUrl} placeholder="socks5://127.0.0.1:12334"/></label></div><p class="fine-print">This registers an existing authorized session. Login verification remains in the primary connection setup above; never reuse one session file concurrently.</p><button class="primary" on:click={addAccount} disabled={busy || !label || !apiId || !apiHash || !storageChatId}>{busy ? 'Saving…' : 'Save connection'}</button></div>
  {:else if accountsTab === 'replication'}
    <div class="card surface"><div class="section-head"><div><p class="card-label">Replication policy</p><h3>Copy or grant bucket access</h3></div></div><div class="form-grid"><label>Source account<select bind:value={sourceId}>{#each accounts as account}<option value={account.id}>{account.label}</option>{/each}</select></label><label>Target account<select bind:value={targetId}>{#each accounts as account}<option value={account.id}>{account.label}</option>{/each}</select></label><label>Bucket<select bind:value={bucket}><option value="">Choose a bucket</option>{#each availableBuckets as item}<option value={item.name}>{item.name}</option>{/each}</select></label><label>Schedule<select bind:value={mode}><option value="one_time">One-time replicate</option><option value="automatic">Automatic replication</option></select></label><label>Target mode<select bind:value={accessMode}><option value="replica">Physical replica</option><option value="access">Access-only via shared chat</option></select></label></div><p class="fine-print">Use the replica user badge in Buckets to scope a job to selected files. This form handles the complete bucket. Physical replicas upload durable copies; access-only entries require the target session to see the source group.</p><button class="primary" on:click={createReplication} disabled={busy || !sourceId || !targetId || sourceId === targetId || !selectedBucket}>{busy ? 'Queueing…' : 'Queue bucket replication'}</button></div>
    {@render jobList(replication, [], 'No replication jobs yet. Open a file or bulk selection in Buckets to queue a scoped copy.')}
  {:else}
    <div class="card surface maintenance-intro"><p class="card-label">Worker queues</p><h3>Maintenance progress</h3><p class="fine-print">Replication and re-chunk jobs are durable. Objects being re-chunked remain unavailable until their worker finishes.</p></div>
    {@render jobList(replication, rechunk, 'No account or re-chunk jobs yet.')}
  {/if}
</section>

{#snippet jobList(replication: ReplicationJob[], rechunk: RechunkJob[], empty: string)}
  <div class="card surface"><div class="job-list">{#if !replication.length && !rechunk.length}<p class="empty">{empty}</p>{:else}{#each replication as job}<div class="job-row"><div><strong>{job.bucket}</strong><small>{job.object_keys?.length ? `${job.object_keys.length} selected object(s) · ` : ''}{job.mode} · {job.access_mode} · {job.state}</small></div><div class="job-progress"><progress max={Math.max(job.chunks_total, 1)} value={job.chunks_done}></progress><small>{job.chunks_done}/{job.chunks_total} chunks · {formatBytes(job.bytes_done)}</small></div></div>{/each}{#each rechunk as job}<div class="job-row"><div><strong>{job.key}</strong><small>re-chunk to {formatBytes(job.new_chunk_size)} · {job.state}</small></div><div class="job-progress"><progress max={Math.max(job.chunks_total, 1)} value={job.chunks_done}></progress><small>{job.chunks_done}/{job.chunks_total} chunks · {formatBytes(job.bytes_done)}</small></div></div>{/each}{/if}</div></div>
{/snippet}

<style>
  .page-grid{display:grid;gap:18px}.hero{display:flex;justify-content:space-between;gap:20px;align-items:flex-start}.hero h2,.card h3{margin:.25rem 0;color:#203b57}.count-pill,.state{display:inline-flex;align-items:center;padding:7px 11px;border-radius:999px;background:#e5f5f0;color:#17604a;font-size:.75rem;font-weight:800;white-space:nowrap}.account-tabs{display:flex;gap:10px;padding:6px;border:1px solid #d7e2ee;border-radius:18px;background:#f5f8fc;box-shadow:0 8px 24px rgba(23,43,77,.05)}.account-tabs button{display:flex;align-items:center;gap:11px;flex:1;padding:11px 14px;border:1px solid transparent;border-radius:13px;background:transparent;color:#5d718b;text-align:left}.account-tabs button.active{border-color:#cbdced;background:#fff;color:#17345a;box-shadow:0 5px 14px rgba(23,58,96,.08)}.account-tabs button:hover:not(:disabled){background:#fff;color:#17345a}.account-tabs strong,.account-tabs small{display:block}.account-tabs strong{font-size:.86rem}.account-tabs small{margin-top:2px;color:#8191a5;font-size:.72rem}.tab-icon{display:grid;place-items:center;width:32px;height:32px;border-radius:10px;background:#eaf3ff;color:#2874b7;font-size:1.1rem;font-weight:800}.tab-icon.green{background:#e8f7f1;color:#1f8b69}.tab-icon.amber{background:#fff3d6;color:#bd791e}.notice{padding:12px 16px;border:1px solid #bde4d2;border-radius:12px;background:#f1fcf6;color:#17604a}.account-list,.job-list{display:grid;gap:9px}.account-row,.job-row{display:flex;align-items:center;gap:13px;padding:13px;border:1px solid #e0eaf1;border-radius:13px;background:#fbfdff}.account-icon{display:grid;place-items:center;flex:0 0 auto;width:38px;height:38px;border-radius:12px;background:#dceefe;color:#216c9e;font-weight:850}.account-copy{display:grid;gap:3px;min-width:0;flex:1}.account-copy strong,.job-row strong{color:#203b57}.account-copy small,.job-row small{color:var(--muted);font-size:.72rem;overflow-wrap:anywhere}.primary-tag{margin-left:8px;padding:3px 7px;border-radius:999px;background:#eaf3ff;color:#28679d;font-size:.62rem;font-weight:800}.danger{color:#a63333}.form-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:13px;margin-bottom:14px}.form-grid label{display:grid;gap:6px;color:#426079;font-size:.75rem;font-weight:800}.form-grid input,.form-grid select{width:100%;box-sizing:border-box;padding:10px 12px;border:1px solid var(--border);border-radius:10px;background:#fff;color:var(--ink);font:inherit;font-weight:500}.form-grid .wide{grid-column:1/-1}.job-row{justify-content:space-between}.job-progress{display:grid;gap:4px;min-width:180px}.job-progress progress{width:100%;accent-color:var(--accent)}.empty{padding:18px 0;color:var(--muted)}.maintenance-intro{padding:22px}@media(max-width:700px){.hero,.account-row,.job-row{align-items:stretch;flex-direction:column}.account-tabs{overflow:auto}.account-tabs button{min-width:170px}.form-grid{grid-template-columns:1fr}.form-grid .wide{grid-column:auto}.job-progress{min-width:0}}
</style>
