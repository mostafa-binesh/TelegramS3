<script lang="ts">
  import {onMount} from 'svelte';
  import {deleteAccount, listAccounts, listBuckets, listReplicationJobs, listRechunkJobs, queueReplication, saveAccount} from '../lib/api';
  import {formatBytes} from '../lib/format';
  import type {AccountInfo, ReplicationJob, RechunkJob} from '../lib/types';
  import type {AccountsTab} from '../lib/router';
  import LoadError from './LoadError.svelte';

  export let csrf: string | null | undefined;
  export let buckets: {name: string}[] = [];
  export let accountsTab: AccountsTab = 'connections';
  export let onTabChange: (tab: AccountsTab) => void = () => {};
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

  type AccountDraft = {label: string; phone: string; apiId: string; apiHash: string; sessionPath: string; storageChatId: string; proxyUrl: string; proxyUsername: string; proxyPassword: string; proxyMode: string; downloadEnabled: boolean};
  const emptyDraft = (): AccountDraft => ({label: '', phone: '', apiId: '', apiHash: '', sessionPath: '', storageChatId: '', proxyUrl: '', proxyUsername: '', proxyPassword: '', proxyMode: 'auto', downloadEnabled: true});
  let availableBuckets: {name: string}[] = buckets;
  let accounts: AccountInfo[] = [];
  let replication: ReplicationJob[] = [];
  let rechunk: RechunkJob[] = [];
  let loading = true;
  let busy = false;
  let error = '';
  let message = '';
  let selectedAccountId: string | null = null;
  let selectionTouched = false;
  let draft: AccountDraft = emptyDraft();
  let draftDirty = false;
  let sourceId = '';
  let targetId = '';
  let bucket = '';
  let mode: 'one_time'|'automatic' = 'one_time';
  let accessMode: 'replica'|'access' = 'replica';
  let showRemoveConnection = false;
  let deleteUploadedFiles = false;
  let phoneConfirmation = '';
  let removeBusy = false;
  let removeError = '';

  $: selectedAccount = accounts.find((account) => account.id === selectedAccountId) ?? null;
  $: isPrimary = Boolean(selectedAccount && accounts[0]?.id === selectedAccount.id);
  $: isAdding = !selectedAccount;
  $: selectedBucket = bucket || availableBuckets[0]?.name || '';
  $: accountCountLabel = `${accounts.length} account${accounts.length === 1 ? '' : 's'}`;
  $: selectedHealth = overview?.telegram?.accounts?.find((account: any) => account.id === selectedAccountId);

  function primaryDraft(): AccountDraft {
    return {label: accounts[0]?.label || 'Primary account', phone: telegramAccountPhone, apiId: telegramApiId, apiHash: telegramApiHash, sessionPath: '', storageChatId: telegramStorageChatId, proxyUrl: telegramProxyUrl, proxyUsername: telegramProxyUsername, proxyPassword: telegramProxyPassword, proxyMode: telegramProxyMode || 'auto', downloadEnabled: accounts[0]?.download_enabled ?? true};
  }

  function selectAccount(account: AccountInfo | null) {
    selectionTouched = true;
    selectedAccountId = account?.id ?? null;
    draftDirty = false;
    onToggleWizard(false);
    if (!account) draft = emptyDraft();
    else if (accounts[0]?.id === account.id) draft = primaryDraft();
    else draft = {label: account.label, phone: account.phone || '', apiId: '', apiHash: '', sessionPath: '', storageChatId: account.storage_chat_id || '', proxyUrl: '', proxyUsername: '', proxyPassword: '', proxyMode: 'auto', downloadEnabled: account.download_enabled};
  }

  function markDraftDirty() { draftDirty = true; }
  function applyPrimaryDraft() {
    telegramAccountPhone = draft.phone; telegramApiId = draft.apiId; telegramApiHash = draft.apiHash; telegramStorageChatId = draft.storageChatId; telegramProxyUrl = draft.proxyUrl; telegramProxyUsername = draft.proxyUsername; telegramProxyPassword = draft.proxyPassword; telegramProxyMode = draft.proxyMode;
  }

  async function refresh() {
    loading = true;
    try {
      const [a, r, c, b] = await Promise.all([listAccounts(csrf), listReplicationJobs(csrf), listRechunkJobs(csrf), listBuckets(csrf, {page: 1, pageSize: 100})]);
      const legacyPrimary: AccountInfo = {id: 'primary', label: 'Primary account', phone: telegramAccountPhone || null, state: overview?.telegram?.connection_state || 'configured', storage_chat_id: telegramStorageChatId || overview?.telegram?.storage_chat_id || null, replica_objects: 0, access_objects: 0, download_enabled: true, created_at: 0, updated_at: 0};
      accounts = a.accounts?.length ? a.accounts : (telegramApiId || telegramStorageChatId || telegramAccountPhone ? [legacyPrimary] : []);
      replication = r.jobs; rechunk = c.jobs; availableBuckets = b.buckets;
      if (!selectionTouched && !selectedAccountId && accounts[0]) selectAccount(accounts[0]);
      else if (selectedAccountId && !accounts.some((account) => account.id === selectedAccountId)) selectAccount(accounts[0] ?? null);
      else if (!draftDirty && isPrimary) draft = primaryDraft();
      if (!sourceId && accounts[0]) sourceId = accounts[0].id;
      if (!targetId && accounts[1]) targetId = accounts[1].id;
      error = '';
    } catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to load account state'; }
    finally { loading = false; }
  }

  async function saveEditor() {
    if (!draft.label.trim() || (!selectedAccount && (!draft.apiId.trim() || !draft.apiHash.trim() || !draft.storageChatId.trim()))) return;
    busy = true; message = ''; error = '';
    try {
      if (isPrimary) applyPrimaryDraft();
      const result = await saveAccount(csrf, {id: selectedAccountId || undefined, label: draft.label.trim(), phone: draft.phone.trim() || undefined, telegram_api_id: draft.apiId.trim() || undefined, telegram_api_hash: draft.apiHash || undefined, telegram_session_path: draft.sessionPath.trim() || undefined, telegram_storage_chat_id: draft.storageChatId.trim() || undefined, telegram_proxy_url: draft.proxyUrl.trim() || undefined, telegram_proxy_username: draft.proxyUsername.trim() || undefined, telegram_proxy_password: draft.proxyPassword || undefined, telegram_proxy_mode: draft.proxyMode, download_enabled: draft.downloadEnabled});
      selectedAccountId = result.account.id; draftDirty = false;
      message = result.refresh_error ? `Account saved. Connection refresh warning: ${result.refresh_error}` : 'Account saved. Its connection policy is active immediately.';
      await refresh();
    } catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to save account'; }
    finally { busy = false; }
  }

  async function removeAccount(account: AccountInfo) {
    if (!confirm(`Remove ${account.label}? Existing files are not deleted.`)) return;
    busy = true; error = '';
    try { await deleteAccount(csrf, account.id); selectAccount(accounts.find((item) => item.id !== account.id) ?? null); await refresh(); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to remove connection'; }
    finally { busy = false; }
  }

  $: phoneConfirmationMatches = Boolean(draft.phone.trim()) && phoneConfirmation.trim() === draft.phone.trim();

  function openRemoval() {
    removeError = '';
    deleteUploadedFiles = false;
    phoneConfirmation = '';
    showRemoveConnection = true;
  }

  async function removePrimary() {
    removeBusy = true;
    removeError = '';
    try {
      await onRemoveConnection(phoneConfirmation, deleteUploadedFiles);
      showRemoveConnection = false;
      await refresh();
    } catch (cause) {
      removeError = cause instanceof Error ? cause.message : 'Connection removal failed';
    } finally {
      removeBusy = false;
    }
  }

  async function createReplication() {
    busy = true; message = ''; error = '';
    try { await queueReplication(csrf, {source_account_id: sourceId, target_account_id: targetId, bucket: selectedBucket, mode, access_mode: accessMode}); message = 'Replication job queued. Progress is durable and survives a restart.'; await refresh(); }
    catch (cause) { error = cause instanceof Error ? cause.message : 'Unable to queue replication'; }
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
  <div class="card surface hero"><div><p class="card-label">Accounts and connections</p><h2>Telegram account pool</h2><p class="fine-print">Choose an account to edit it, or use the add tile to register another transport. Download eligibility is independent from ownership and cleanup.</p></div><span class="count-pill">{accountCountLabel}</span></div>
  <div class="account-tabs" role="tablist" aria-label="Account management sections">
    <button class:active={accountsTab === 'connections'} type="button" role="tab" aria-selected={accountsTab === 'connections'} on:click={() => onTabChange('connections')}><span class="tab-icon">⌁</span><span><strong>Connections</strong><small>Accounts and health</small></span></button>
    <button class:active={accountsTab === 'replication'} type="button" role="tab" aria-selected={accountsTab === 'replication'} on:click={() => onTabChange('replication')}><span class="tab-icon green">⇄</span><span><strong>Replication</strong><small>Copies and access</small></span></button>
    <button class:active={accountsTab === 'maintenance'} type="button" role="tab" aria-selected={accountsTab === 'maintenance'} on:click={() => onTabChange('maintenance')}><span class="tab-icon amber">◈</span><span><strong>Maintenance</strong><small>Worker progress</small></span></button>
  </div>
  {#if error}<LoadError title="Account service unavailable" message={error} onRetry={refresh}/>{/if}
  {#if message}<div class="notice" role="status">{message}</div>{/if}

  {#if accountsTab === 'connections'}
    <section class="card surface connections-workspace">
      <div class="section-head"><div><p class="card-label">Account switcher</p><h3>Select a connection</h3></div><span class="fine-print">{accounts.length ? 'Each card opens the same editor' : 'Start with your first account'}</span></div>
      {#if loading}<div class="skeleton-stack"><div class="skeleton" style="height:86px"></div><div class="skeleton" style="height:86px"></div></div>{:else}<div class="account-card-grid" role="tablist" aria-label="Telegram accounts">
        {#each accounts as account, index (account.id)}
          {@const health = overview?.telegram?.accounts?.find((item: any) => item.id === account.id)}
          <button class:chosen={selectedAccountId === account.id} class="account-card" type="button" role="tab" aria-selected={selectedAccountId === account.id} on:click={() => selectAccount(account)}><span class="account-card-top"><span class="account-icon">{account.label.slice(0, 1).toUpperCase()}</span><span class="health-dot" class:online={health?.connected} title={health?.detail || account.state}></span></span><span class="account-card-copy"><strong>{account.label}</strong>{#if index === 0}<small class="primary-tag">Primary</small>{/if}<small>{account.phone || 'Phone hidden'}</small><small>{account.download_enabled ? 'Available for downloads' : 'Download disabled'}</small></span><span class="account-card-arrow">→</span></button>
        {/each}
        <button class="account-card add-card" type="button" role="tab" aria-selected={isAdding} on:click={() => selectAccount(null)}><span class="add-mark">＋</span><span><strong>Add account</strong><small>Register another Telegram transport</small></span></button>
      </div>{/if}
    </section>

    {#if showWizard && wizardComponent && isPrimary}
      <svelte:component this={wizardComponent} csrf={session?.csrf_token} bind:telegramApiId bind:telegramApiHash bind:telegramStorageChatId bind:telegramProxyUrl bind:telegramProxyUsername bind:telegramProxyPassword bind:telegramProxyMode {settingsBusy} {settingsError} {settingsMessage} {onSave} onDone={onWizardDone} onClose={onWizardClose}/>
    {:else}
      <section class="card surface editor-card" aria-label={isAdding ? 'Add account' : `Edit ${selectedAccount?.label ?? 'account'}`}>
        <div class="section-head"><div><p class="card-label">{isAdding ? 'New account' : 'Account editor'}</p><h2>{isAdding ? 'Add a Telegram account' : isPrimary ? 'Your storage connection, beautifully in sync.' : `Edit ${selectedAccount?.label}`}</h2><p class="fine-print">{isAdding ? 'Use a separate authorized session file when adding the same Telegram account again.' : 'The same form is used for creating and changing every account. Leave secret fields blank to keep the stored value.'}</p></div>{#if isPrimary}<button class="ghost" type="button" on:click={() => onToggleWizard(true)}>Edit account setup <span aria-hidden="true">→</span></button>{/if}</div>
        <div class="form-grid"><label>Account label<input bind:value={draft.label} on:input={markDraftDirty} placeholder="Primary account" /></label><label>Phone (optional)<input bind:value={draft.phone} on:input={markDraftDirty} placeholder="+989…" /></label><label>Telegram API ID<input bind:value={draft.apiId} on:input={markDraftDirty} inputmode="numeric" placeholder={isAdding ? '123456' : 'Leave unchanged'} /></label><label>Telegram API hash<input bind:value={draft.apiHash} on:input={markDraftDirty} type="password" placeholder={isAdding ? 'Application hash' : 'Leave unchanged'} /></label><label>Session file<input bind:value={draft.sessionPath} on:input={markDraftDirty} placeholder={isAdding ? 'data/telegram-account-2.session' : 'Leave unchanged'} /></label><label>Storage chat ID<input bind:value={draft.storageChatId} on:input={markDraftDirty} placeholder={isAdding ? '-100…' : 'Leave unchanged'} /></label><label>Proxy URL<input bind:value={draft.proxyUrl} on:input={markDraftDirty} placeholder="socks5://127.0.0.1:12334" /></label><label>Proxy mode<select bind:value={draft.proxyMode} on:change={markDraftDirty}><option value="auto">Automatic</option><option value="socks5">SOCKS5</option><option value="disabled">Direct / disabled</option></select></label><label>Proxy username<input bind:value={draft.proxyUsername} on:input={markDraftDirty} placeholder="Optional" /></label><label>Proxy password<input bind:value={draft.proxyPassword} on:input={markDraftDirty} type="password" placeholder="Optional" /></label></div>
        <label class="download-policy"><input type="checkbox" bind:checked={draft.downloadEnabled} on:change={markDraftDirty}/><span><strong>Use this account for downloads</strong><small>Turn this off to keep the account available for ownership and cleanup while reads use other replicas.</small></span></label>
        {#if settingsMessage || settingsError}<div class:message-error={Boolean(settingsError)} class="editor-message" role={settingsError ? 'alert' : 'status'}>{settingsError || settingsMessage}</div>{/if}
        <div class="editor-actions"><span class="fine-print">{isPrimary ? 'Primary setup and authorization remain available above.' : 'Credentials are preserved when an edit leaves secret fields blank.'}</span><div>{#if isPrimary}<button class="ghost" type="button" on:click={onManageOperators}>Manage operators</button><button class="ghost danger" type="button" on:click={openRemoval} disabled={busy || !draft.phone.trim()}>Remove connection</button>{/if}<button class="primary" type="button" on:click={saveEditor} disabled={busy || !draft.label.trim() || (!selectedAccount && (!draft.apiId.trim() || !draft.apiHash.trim() || !draft.storageChatId.trim()))}>{busy ? 'Saving…' : isAdding ? 'Add account' : 'Save changes'}</button>{#if selectedAccount && !isPrimary}<button class="ghost danger" type="button" on:click={() => removeAccount(selectedAccount)} disabled={busy}>Remove account</button>{/if}</div></div>
      </section>
    {/if}
  {:else if accountsTab === 'replication'}
    <div class="card surface"><div class="section-head"><div><p class="card-label">Replication policy</p><h3>Copy or grant bucket access</h3></div></div><div class="form-grid"><label>Source account<select bind:value={sourceId}>{#each accounts as account}<option value={account.id}>{account.label}</option>{/each}</select></label><label>Target account<select bind:value={targetId}>{#each accounts as account}<option value={account.id}>{account.label}</option>{/each}</select></label><label>Bucket<select bind:value={bucket}><option value="">Choose a bucket</option>{#each availableBuckets as item}<option value={item.name}>{item.name}</option>{/each}</select></label><label>Schedule<select bind:value={mode}><option value="one_time">One-time replicate</option><option value="automatic">Automatic replication</option></select></label><label>Target mode<select bind:value={accessMode}><option value="replica">Physical replica</option><option value="access">Access-only via shared chat</option></select></label></div><p class="fine-print">Physical replicas add alternate download locations. Access-only entries use the shared Telegram chat through the target account.</p><button class="primary" on:click={createReplication} disabled={busy || !sourceId || !targetId || sourceId === targetId || !selectedBucket}>{busy ? 'Queueing…' : 'Queue bucket replication'}</button></div>
    {@render jobList(replication, [], 'No replication jobs yet.')}
  {:else}
    <div class="card surface maintenance-intro"><p class="card-label">Worker queues</p><h3>Maintenance progress</h3><p class="fine-print">Replication and re-chunk jobs are durable. Objects being re-chunked remain unavailable until their worker finishes.</p></div>
    {@render jobList(replication, rechunk, 'No account or re-chunk jobs yet.')}
  {/if}
</section>

{#if showRemoveConnection}
  <div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && !removeBusy && (showRemoveConnection = false)}>
    <div class="modal-card compact-modal" role="dialog" aria-modal="true" aria-label="Remove current connection">
      <form on:submit|preventDefault={removePrimary}>
      <div class="section-head"><div><p class="card-label">Telegram connection</p><h2>Remove current connection?</h2></div><button class="icon-button" type="button" aria-label="Close" on:click={() => showRemoveConnection = false} disabled={removeBusy}>×</button></div>
      <p class="fine-print">The dashboard will remove this connection from the console and optionally schedule remote file cleanup.</p>
      {#if draft.phone.trim()}<div class="account-confirmation"><span class="card-label">Account selected for removal</span><strong>{draft.phone}</strong><span>Type this exact number below to confirm.</span></div><label><span>Type the displayed account number</span><input aria-label="Type the displayed account number" type="text" bind:value={phoneConfirmation} autocomplete="off" placeholder={draft.phone} /></label>{:else}<p class="error-hint" role="alert">The Telegram account number is unavailable. Reauthorize the connection before removing it.</p>{/if}
      <label class="checkbox-row"><input type="checkbox" bind:checked={deleteUploadedFiles} /><span>Also delete all uploaded Telegram files</span></label>
      <p class="fine-print">If unchecked, uploaded Telegram files remain in Telegram but this connection will no longer manage them. If checked, deletion runs in the background.</p>
      {#if removeError}<p class="error-hint" role="alert">{removeError}</p>{/if}
      <div class="settings-actions"><button class="ghost" type="button" on:click={() => showRemoveConnection = false} disabled={removeBusy}>Cancel</button><button class="danger-button" type="submit" disabled={removeBusy || !phoneConfirmationMatches}>{removeBusy ? 'Removing…' : 'Remove connection'}</button></div>
      </form>
    </div>
  </div>
{/if}

{#snippet jobList(replication: ReplicationJob[], rechunk: RechunkJob[], empty: string)}
  <div class="card surface"><div class="job-list">{#if !replication.length && !rechunk.length}<p class="empty">{empty}</p>{:else}{#each replication as job}<div class="job-row"><div><strong>{job.bucket}</strong><small>{job.object_keys?.length ? `${job.object_keys.length} selected object(s) · ` : ''}{job.mode} · {job.access_mode} · {job.state}</small></div><div class="job-progress"><progress max={Math.max(job.chunks_total, 1)} value={job.chunks_done}></progress><small>{job.chunks_done}/{job.chunks_total} chunks · {formatBytes(job.bytes_done)}</small></div></div>{/each}{#each rechunk as job}<div class="job-row"><div><strong>{job.key}</strong><small>re-chunk to {formatBytes(job.new_chunk_size)} · {job.state}</small></div><div class="job-progress"><progress max={Math.max(job.chunks_total, 1)} value={job.chunks_done}></progress><small>{job.chunks_done}/{job.chunks_total} chunks · {formatBytes(job.bytes_done)}</small></div></div>{/each}{/if}</div></div>
{/snippet}

<style>
  .page-grid{display:grid;gap:18px}.hero{display:flex;justify-content:space-between;gap:20px;align-items:flex-start}.hero h2,.card h2,.card h3{margin:.25rem 0;color:#203b57}.count-pill{display:inline-flex;align-items:center;padding:7px 11px;border-radius:999px;background:#e5f5f0;color:#17604a;font-size:.75rem;font-weight:800;white-space:nowrap}.account-tabs{display:flex;gap:10px;padding:6px;border:1px solid #d7e2ee;border-radius:18px;background:#f5f8fc;box-shadow:0 8px 24px rgba(23,43,77,.05)}.account-tabs button{display:flex;align-items:center;gap:11px;flex:1;padding:11px 14px;border:1px solid transparent;border-radius:13px;background:transparent;color:#5d718b;text-align:left}.account-tabs button.active{border-color:#cbdced;background:#fff;color:#17345a;box-shadow:0 5px 14px rgba(23,58,96,.08)}.account-tabs button:hover:not(:disabled){background:#fff;color:#17345a}.account-tabs strong,.account-tabs small{display:block}.account-tabs strong{font-size:.86rem}.account-tabs small{margin-top:2px;color:#8191a5;font-size:.72rem}.tab-icon{display:grid;place-items:center;width:32px;height:32px;border-radius:10px;background:#eaf3ff;color:#2874b7;font-size:1.1rem;font-weight:800}.tab-icon.green{background:#e8f7f1;color:#1f8b69}.tab-icon.amber{background:#fff3d6;color:#bd791e}.notice{padding:12px 16px;border:1px solid #bde4d2;border-radius:12px;background:#f1fcf6;color:#17604a}.section-head{align-items:flex-start}.account-card-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px}.account-card{position:relative;display:flex;align-items:flex-start;gap:12px;min-height:116px;padding:15px;border:1px solid #dce7f0;border-radius:16px;background:linear-gradient(145deg,#fff,#f8fbfe);color:inherit;text-align:left;cursor:pointer;transition:transform 140ms ease,box-shadow 140ms ease,border-color 140ms ease}.account-card:hover,.account-card.chosen{border-color:#8dc3dc;box-shadow:0 12px 26px rgba(29,91,132,.12);transform:translateY(-1px)}.account-card.chosen{background:#f2faff}.account-card-top{display:flex;flex-direction:column;align-items:center;gap:10px}.account-icon{display:grid;place-items:center;width:42px;height:42px;border-radius:14px;background:#dceefe;color:#216c9e;font-weight:850;font-size:1.1rem}.health-dot{width:9px;height:9px;border-radius:50%;background:#c64747;box-shadow:0 0 0 3px #fff}.health-dot.online{background:#168466}.account-card-copy{display:grid;gap:5px;min-width:0}.account-card-copy strong{color:#203b57;overflow-wrap:anywhere}.account-card-copy small{color:var(--muted);font-size:.72rem;overflow-wrap:anywhere}.primary-tag{color:#28679d!important;font-weight:800}.account-card-arrow{margin-left:auto;color:#3a8ab7;font-weight:850}.add-card{align-items:center;justify-content:center;border-style:dashed;background:#fbfdff}.add-mark{display:grid;place-items:center;width:42px;height:42px;border-radius:14px;background:#e8f7f1;color:#1f8b69;font-size:1.55rem}.editor-card{padding:22px}.editor-card>.section-head{margin-bottom:20px}.editor-card h2{max-width:760px}.form-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:13px;margin-bottom:15px}.form-grid label{display:grid;gap:6px;color:#426079;font-size:.75rem;font-weight:800}.form-grid input,.form-grid select{width:100%;box-sizing:border-box;padding:10px 12px;border:1px solid var(--border);border-radius:10px;background:#fff;color:var(--ink);font:inherit;font-weight:500}.download-policy{display:flex;align-items:flex-start;gap:10px;padding:14px;border:1px solid #dce7f0;border-radius:12px;background:#f8fbfe;color:#203b57}.download-policy input{margin-top:3px;accent-color:#287abe}.download-policy strong,.download-policy small{display:block}.download-policy small{margin-top:4px;color:var(--muted);font-size:.72rem;font-weight:500}.editor-message{margin-top:13px;color:#17604a}.message-error{color:#a63333}.editor-actions{display:flex;justify-content:space-between;align-items:center;gap:15px;margin-top:17px}.editor-actions>div{display:flex;gap:8px;align-items:center}.danger{color:#a63333}.account-confirmation{display:grid;gap:4px;padding:14px 16px;border:1px solid #f0cccc;border-radius:14px;background:#fff7f7}.account-confirmation strong{font-size:1.1rem;letter-spacing:.02em;color:#7d2020}.account-confirmation>span:last-child{font-size:.8rem;color:var(--muted)}.checkbox-row{display:flex;align-items:center;gap:10px;font-weight:700}.checkbox-row input{flex:0 0 auto;width:18px;height:18px;margin:0;accent-color:#b33838}.checkbox-row span{line-height:1.35}.job-list{display:grid;gap:9px}.job-row{display:flex;align-items:center;justify-content:space-between;gap:13px;padding:13px;border:1px solid #e0eaf1;border-radius:13px;background:#fbfdff}.job-row strong{color:#203b57}.job-row small{display:block;color:var(--muted);font-size:.72rem;overflow-wrap:anywhere}.job-progress{display:grid;gap:4px;min-width:180px}.job-progress progress{width:100%;accent-color:var(--accent)}.empty{padding:18px 0;color:var(--muted)}.maintenance-intro{padding:22px}
  @media(max-width:700px){.hero,.job-row{align-items:stretch;flex-direction:column}.account-tabs{overflow:auto}.account-tabs button{min-width:170px}.form-grid{grid-template-columns:1fr}.editor-actions{align-items:stretch;flex-direction:column}.editor-actions>div{justify-content:flex-end}.job-progress{min-width:0}}
</style>
