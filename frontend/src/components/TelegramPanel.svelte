<script lang="ts">
  import type { OverviewState, SessionState, TelegramSettings } from '../lib/types';
  import { navigate } from '../lib/router';
  import type { TelegramTab } from '../lib/router';
  export let tab: TelegramTab = 'connection';
  export let overview: OverviewState | null = null;
  export let session: SessionState | null = null;
  export let telegramApiId = '';
  export let telegramApiHash = '';
  export let telegramStorageChatId = '';
  export let telegramProxyUrl = '';
  export let telegramProxyUsername = '';
  export let telegramProxyPassword = '';
  export let telegramProxyMode = 'auto';
  export let settingsBusy = false;
  export let settingsError = '';
  export let settingsMessage = '';
  export let showCredentials = false;
  export let showWizard = false;
  export let wizardComponent: any = null;
  export let onSave: () => void = () => {};
  export let onManageOperators: () => void = () => {};
  export let onToggleWizard: (open: boolean) => void = () => {};
  export let onWizardDone: () => void = () => {};
  export let onWizardClose: () => void = () => {};
  export let onRemoveConnection: (deleteUploadedFiles: boolean) => Promise<void> = async () => {};
  let showRemoveConnection = false;
  let deleteUploadedFiles = false;
  let removeBusy = false;
  let removeError = '';

  $: needsSetup = (overview?.telegram?.connection_state ?? 'needs_reauth') !== 'connected';
  $: hasConnection = !needsSetup || Boolean(telegramApiId || telegramStorageChatId || overview?.telegram?.storage_chat_id);
  $: statusLabel = overview?.telegram?.connection_state?.replaceAll('_', ' ') ?? 'needs reauth';
</script>

<article class="card surface tg-callout">
  <div class="subtabs" role="tablist" aria-label="Telegram settings views">
    <button class:active={tab === 'connection'} role="tab" aria-selected={tab === 'connection'} on:click={() => navigate({view: 'telegram', telegramTab: 'connection'})}>Connection</button>
    <button class:active={tab === 'proxy'} role="tab" aria-selected={tab === 'proxy'} on:click={() => navigate({view: 'telegram', telegramTab: 'proxy'})}>Proxy</button>
  </div>
  {#if tab === 'connection'}
    <div class="tg-banner"><div class="tg-copy"><p class="card-label">Telegram</p><h2>{needsSetup ? 'Telegram storage is not connected' : 'Telegram storage is connected'}</h2><p class="fine-print">{statusLabel} · {overview?.telegram?.detail ?? 'No Telegram status available.'}</p></div>
      <div class="tg-actions"><button class="primary" type="button" on:click={() => onToggleWizard(true)}>{needsSetup ? 'Set up Telegram login' : 'Refresh Telegram login'}</button><button class="ghost" type="button" on:click={onManageOperators}>Manage operators</button>{#if hasConnection}<button class="danger-button" type="button" on:click={() => { removeError = ''; showRemoveConnection = true; }}>Remove current connection</button>{/if}</div>
    </div>
    <div class="settings-grid settings-summary">
      <article class="settings-card"><p class="card-label">Credentials</p><p class="fine-print">API credentials and storage chat stay in a protected dialog.</p><button class="primary" type="button" on:click={() => showCredentials = true}>Edit credentials</button></article>
    </div>
  {:else}
    <article class="settings-card"><p class="card-label">Proxy</p><p class="fine-print">Use a proxy only when your network requires it.</p><form class="proxy-form" on:submit|preventDefault={onSave}><label><span>Proxy mode</span><select bind:value={telegramProxyMode}><option value="auto">Auto</option><option value="disabled">Disabled</option><option value="socks5">SOCKS5</option><option value="http">HTTP</option></select></label><label><span>Proxy URL</span><input bind:value={telegramProxyUrl} placeholder="socks5://127.0.0.1:12334" /></label><div class="grid-2"><label><span>Username</span><input bind:value={telegramProxyUsername} /></label><label><span>Password</span><input bind:value={telegramProxyPassword} type="password" /></label></div><button class="primary" type="submit" disabled={settingsBusy}>Save proxy settings</button></form></article>
  {/if}
  {#if settingsMessage}<p class="fine-print">{settingsMessage}</p>{/if}{#if settingsError}<p class="fine-print error-hint">{settingsError}</p>{/if}
</article>

{#if showWizard && wizardComponent}<svelte:component this={wizardComponent} csrf={session?.csrf_token} onDone={onWizardDone} onClose={onWizardClose}/>{/if}

{#if showRemoveConnection}
  <div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && !removeBusy && (showRemoveConnection = false)}>
    <form class="modal-card compact-modal" on:submit|preventDefault={async () => { removeBusy = true; removeError = ''; try { await onRemoveConnection(deleteUploadedFiles); showRemoveConnection = false; } catch (cause) { removeError = cause instanceof Error ? cause.message : 'Connection removal failed'; } finally { removeBusy = false; } }}>
      <div class="section-head"><div><p class="card-label">Telegram connection</p><h2>Remove current connection?</h2></div><button class="icon-button" type="button" aria-label="Close" on:click={() => showRemoveConnection = false} disabled={removeBusy}>×</button></div>
      <p class="fine-print">The dashboard will remove its buckets, files, and statistics immediately. This cannot be undone from this installation.</p>
      <label class="checkbox-row"><input type="checkbox" bind:checked={deleteUploadedFiles} /><span>Also delete all uploaded Telegram files</span></label>
      <p class="fine-print">If unchecked, uploaded Telegram files remain in Telegram but this connection will no longer manage them. If checked, deletion runs in the background and may take time.</p>
      {#if removeError}<p class="error-hint">{removeError}</p>{/if}
      <div class="settings-actions"><button class="ghost" type="button" on:click={() => showRemoveConnection = false} disabled={removeBusy}>Cancel</button><button class="danger-button" type="submit" disabled={removeBusy}>{removeBusy ? 'Removing…' : 'Remove connection'}</button></div>
    </form>
  </div>
{/if}

{#if showCredentials}
  <div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showCredentials = false)}><form class="modal-card" on:submit|preventDefault={() => { showCredentials = false; onSave(); }}><div class="section-head"><div><p class="card-label">Telegram credentials</p><h2>Storage account</h2></div><button class="icon-button" type="button" on:click={() => showCredentials = false}>×</button></div><div class="settings-grid"><label><span>Telegram API ID</span><input bind:value={telegramApiId} autocomplete="off" /></label><label><span>Telegram API hash</span><input bind:value={telegramApiHash} type="password" autocomplete="off" /></label><label><span>Storage chat ID</span><input bind:value={telegramStorageChatId} autocomplete="off" /></label></div><div class="settings-actions"><button class="primary" type="submit" disabled={settingsBusy}>Save credentials</button></div></form></div>
{/if}

<style>
  .checkbox-row{display:flex;align-items:flex-start;gap:10px;font-weight:700}.checkbox-row input{width:18px;height:18px;accent-color:var(--danger)}.danger-button{background:var(--danger);color:#fff;border-color:var(--danger)}.danger-button:hover:not(:disabled){background:#8f2929}
</style>
