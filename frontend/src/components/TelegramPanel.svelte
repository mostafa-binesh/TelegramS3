<script lang="ts">
  import type { OverviewState, SessionState, TelegramSettings } from '../lib/types';
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

  $: needsSetup = (overview?.telegram?.connection_state ?? 'needs_reauth') !== 'connected';
  $: statusLabel = overview?.telegram?.connection_state?.replaceAll('_', ' ') ?? 'needs reauth';
</script>

<article class="card surface tg-callout">
  <div class="tg-banner"><div class="tg-copy"><p class="card-label">Telegram</p><h2>{needsSetup ? 'Telegram storage is not connected' : 'Telegram storage is connected'}</h2><p>One Telegram account backs storage for the whole server. Dashboard operators are managed separately.</p><p class="fine-print">Storage session: {overview?.telegram?.session_state ?? 'Unknown'} • {statusLabel}</p><p class="fine-print">{overview?.telegram?.detail ?? 'No Telegram status available.'}</p></div>
    <div class="tg-actions"><button class="primary" type="button" on:click={() => onToggleWizard(true)}>{needsSetup ? 'Set up Telegram login' : 'Refresh Telegram login'}</button><button class="ghost" type="button" on:click={onManageOperators}>Manage operators</button></div>
  </div>
  <div class="settings-grid settings-summary">
    <article class="settings-card"><p class="card-label">Credentials</p><p class="fine-print">API credentials and storage chat stay in a protected dialog.</p><button class="primary" type="button" on:click={() => showCredentials = true}>Edit credentials</button></article>
    <article class="settings-card"><p class="card-label">Proxy</p><p class="fine-print">Use a proxy only when your network requires it.</p><form class="proxy-form" on:submit|preventDefault={onSave}><label><span>Proxy mode</span><select bind:value={telegramProxyMode}><option value="auto">Auto</option><option value="disabled">Disabled</option><option value="socks5">SOCKS5</option><option value="http">HTTP</option></select></label><label><span>Proxy URL</span><input bind:value={telegramProxyUrl} placeholder="socks5://127.0.0.1:12334" /></label><div class="grid-2"><label><span>Username</span><input bind:value={telegramProxyUsername} /></label><label><span>Password</span><input bind:value={telegramProxyPassword} type="password" /></label></div><button class="primary" type="submit" disabled={settingsBusy}>Save proxy settings</button></form></article>
  </div>
  {#if settingsMessage}<p class="fine-print">{settingsMessage}</p>{/if}{#if settingsError}<p class="fine-print error-hint">{settingsError}</p>{/if}
</article>

{#if showWizard && wizardComponent}<svelte:component this={wizardComponent} csrf={session?.csrf_token} onDone={onWizardDone}/><button class="ghost" type="button" on:click={() => onToggleWizard(false)}>Close wizard</button>{/if}

{#if showCredentials}
  <div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showCredentials = false)}><form class="modal-card" on:submit|preventDefault={() => { showCredentials = false; onSave(); }}><div class="section-head"><div><p class="card-label">Telegram credentials</p><h2>Storage account</h2></div><button class="icon-button" type="button" on:click={() => showCredentials = false}>×</button></div><div class="settings-grid"><label><span>Telegram API ID</span><input bind:value={telegramApiId} autocomplete="off" /></label><label><span>Telegram API hash</span><input bind:value={telegramApiHash} type="password" autocomplete="off" /></label><label><span>Storage chat ID</span><input bind:value={telegramStorageChatId} autocomplete="off" /></label></div><div class="settings-actions"><button class="primary" type="submit" disabled={settingsBusy}>Save credentials</button></div></form></div>
{/if}
