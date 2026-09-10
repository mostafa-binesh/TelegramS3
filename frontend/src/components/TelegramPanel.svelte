<script lang="ts">
  import type { OverviewState, SessionState } from '../lib/types';

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
  export let showWizard = false;
  export let wizardComponent: any = null;
  export let onSave: () => Promise<void> | void = () => {};
  export let onManageOperators: () => void = () => {};
  export let onToggleWizard: (open: boolean) => void = () => {};
  export let onWizardDone: () => void = () => {};
  export let onWizardClose: () => void = () => {};
  export let onRemoveConnection: (phoneConfirmation: string, deleteUploadedFiles: boolean) => Promise<void> = async () => {};
  export let telegramTab: 'connection' | 'proxy' | 'storage' = 'connection';
  export let onTabChange: (tab: 'connection' | 'proxy' | 'storage') => void = () => {};
  export let storageChunkSizeBytes = 1048576;
  export let storageChunkSizeMiB = '1';
  export let storageChunkSizeMin = 1;
  export let storageChunkSizeMax = 2000000000;
  export let storageSettingsBusy = false;
  export let storageSettingsError = '';
  export let storageSettingsMessage = '';
  export let onSaveStorageSettings: () => Promise<void> = async () => {};

  let showRemoveConnection = false;
  let deleteUploadedFiles = false;
  let phoneConfirmation = '';
  let removeBusy = false;
  let removeError = '';

  $: connectionState = overview?.telegram?.connection_state ?? 'needs_reauth';
  $: connected = connectionState === 'connected';
  $: statusLabel = connectionState.replaceAll('_', ' ');
  $: hasConnection = connected || Boolean(telegramApiId || telegramStorageChatId || overview?.telegram?.storage_chat_id);
  $: configuredSteps = [telegramApiId.trim(), telegramStorageChatId.trim(), telegramProxyMode !== 'disabled'].filter(Boolean).length;
  $: maskedApiId = telegramApiId ? `${telegramApiId.slice(0, 2)}•••${telegramApiId.slice(-2)}` : 'Not configured';
  $: maskedChatId = telegramStorageChatId
    ? `${telegramStorageChatId.slice(0, 4)}••••${telegramStorageChatId.slice(-4)}`
    : overview?.telegram?.storage_chat_id
      ? `${overview.telegram.storage_chat_id.slice(0, 4)}••••${overview.telegram.storage_chat_id.slice(-4)}`
      : 'Not configured';
  $: networkLabel = telegramProxyMode === 'disabled'
    ? 'Direct connection'
    : telegramProxyMode === 'auto'
      ? 'Automatic routing'
      : `${telegramProxyMode.toUpperCase()} proxy`;
  $: draftChunkSizeBytes = Math.round(Number(storageChunkSizeMiB) * 1048576);
  $: draftChunkSizeValid = Number.isFinite(draftChunkSizeBytes)
    && draftChunkSizeBytes >= storageChunkSizeMin
    && draftChunkSizeBytes <= storageChunkSizeMax;

  function formatBytes(bytes: number) {
    if (bytes >= 1073741824) return `${(bytes / 1073741824).toFixed(2)} GiB`;
    if (bytes >= 1048576) return `${(bytes / 1048576).toFixed(2)} MiB`;
    if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
    return `${bytes} bytes`;
  }

  function openRemoval() {
    removeError = '';
    deleteUploadedFiles = false;
    phoneConfirmation = '';
    showRemoveConnection = true;
  }

  async function removeConnection() {
    removeBusy = true;
    removeError = '';
    try {
      await onRemoveConnection(phoneConfirmation, deleteUploadedFiles);
      showRemoveConnection = false;
    } catch (cause) {
      removeError = cause instanceof Error ? cause.message : 'Connection removal failed';
    } finally {
      removeBusy = false;
    }
  }
</script>

<div class="settings-tabs" role="tablist" aria-label="Telegram settings sections">
  <button class:active={telegramTab === 'connection'} type="button" role="tab" aria-selected={telegramTab === 'connection'} on:click={() => onTabChange('connection')}><span class="tab-icon">⌁</span><span><strong>Connection</strong><small>Account and health</small></span></button>
  <button class:active={telegramTab === 'storage'} type="button" role="tab" aria-selected={telegramTab === 'storage'} on:click={() => onTabChange('storage')}><span class="tab-icon storage-tab-icon">◈</span><span><strong>Storage policy</strong><small>Chunk size and flow</small></span></button>
</div>

{#if showWizard && wizardComponent}
  <svelte:component
    this={wizardComponent}
    csrf={session?.csrf_token}
    bind:telegramApiId
    bind:telegramApiHash
    bind:telegramStorageChatId
    bind:telegramProxyUrl
    bind:telegramProxyUsername
    bind:telegramProxyPassword
    bind:telegramProxyMode
    {settingsBusy}
    {settingsError}
    {settingsMessage}
    {onSave}
    onDone={onWizardDone}
    onClose={onWizardClose}
  />
{:else if telegramTab === 'storage'}
  <section class="storage-policy-page" aria-labelledby="storage-policy-title">
    <article class="storage-hero">
      <div class="storage-hero-copy">
        <div class="eyebrow-row"><span class="eyebrow">Storage policy</span><span class="live-pill"><span class="live-dot"></span>Live configuration</span></div>
        <h2 id="storage-policy-title">Give every upload the right-sized runway.</h2>
        <p>Chunk size controls how a file is split before it travels to Telegram. Tune the policy for your network and file mix without touching the objects you already have.</p>
      </div>
      <div class="policy-orbit" aria-hidden="true"><div class="policy-core">{formatBytes(storageChunkSizeBytes)}</div><div class="policy-ring ring-a"></div><div class="policy-ring ring-b"></div><span class="policy-spark spark-a"></span><span class="policy-spark spark-b"></span></div>
    </article>

    <div class="policy-grid">
      <form class="policy-card policy-editor" on:submit|preventDefault={onSaveStorageSettings}>
        <div class="policy-card-heading"><div><span class="eyebrow">Upload shaping</span><h3>Chunk size</h3></div><span class="policy-badge">{formatBytes(storageChunkSizeBytes)} now</span></div>
        <p class="policy-description">Larger chunks mean fewer Telegram documents and less metadata. Smaller chunks make retries lighter and keep memory use lower.</p>
        <label class="chunk-input-label"><span>New upload chunk size</span><div class="chunk-input-wrap"><input bind:value={storageChunkSizeMiB} type="number" min={storageChunkSizeMin / 1048576} max={storageChunkSizeMax / 1048576} step="0.25" inputmode="decimal" aria-describedby="chunk-size-help" /><span>MiB</span></div></label>
        <div class="preset-grid" aria-label="Chunk size presets">
          {#each [1, 4, 8, 16, 32] as preset}<button class:chosen={Number(storageChunkSizeMiB) === preset} type="button" on:click={() => storageChunkSizeMiB = String(preset)}>{preset} MiB</button>{/each}
        </div>
        <p id="chunk-size-help" class="range-help">Allowed range: {formatBytes(storageChunkSizeMin)} to {formatBytes(storageChunkSizeMax)}. Recommended starting point: <strong>1–16 MiB</strong>.</p>
        {#if storageSettingsError}<p class="storage-message message-error" role="alert">{storageSettingsError}</p>{/if}
        {#if storageSettingsMessage}<p class="storage-message" role="status">✓ {storageSettingsMessage}</p>{/if}
        <div class="policy-actions"><span class:valid={draftChunkSizeValid} class="draft-preview">{draftChunkSizeValid ? `Will use ${formatBytes(draftChunkSizeBytes)}` : 'Enter a value in the allowed range'}</span><button class="primary" type="submit" disabled={storageSettingsBusy || !draftChunkSizeValid}>{storageSettingsBusy ? 'Applying…' : 'Apply storage policy'}</button></div>
      </form>

      <aside class="policy-card policy-impact">
        <div class="policy-card-heading"><div><span class="eyebrow">What changes</span><h3>Safe by design</h3></div><span class="shield-mark">✓</span></div>
        <div class="impact-list"><div class="impact-row"><span class="impact-icon blue">↗</span><span><strong>New uploads</strong><small>Use the new value immediately, without restarting the server.</small></span></div><div class="impact-row"><span class="impact-icon green">◌</span><span><strong>Existing objects</strong><small>Stay byte-for-byte unchanged and remain fully readable.</small></span></div><div class="impact-row"><span class="impact-icon amber">⏱</span><span><strong>Active transfers</strong><small>Keep the chunk policy they started with, so no transfer changes shape mid-flight.</small></span></div></div>
        <div class="policy-note"><span aria-hidden="true">i</span><span>The value is migrated from <code>TELEGRAM_CHUNK_SIZE</code> the first time metadata opens. After that, this database setting is authoritative.</span></div>
      </aside>
    </div>
  </section>
{:else}
  <section class="telegram-account-page" aria-labelledby="telegram-account-title">
    <article class="account-hero">
      <div class="hero-copy-block">
        <div class="eyebrow-row"><span class="eyebrow">Telegram account</span><span class:connected class="status-pill"><span class="status-dot"></span>{connected ? 'Connected' : 'Needs attention'}</span></div>
        <h2 id="telegram-account-title">Your storage connection, beautifully in sync.</h2>
        <p>Configure the Telegram app, choose where objects live, and authorize the account that powers your S3 storage.</p>
        <div class="hero-actions">
          <button class="primary hero-button" type="button" on:click={() => onToggleWizard(true)}>{connected ? 'Edit account setup' : 'Set up account'}<span aria-hidden="true">→</span></button>
          <button class="hero-link" type="button" on:click={onManageOperators}>Manage operators <span aria-hidden="true">↗</span></button>
        </div>
      </div>
      <div class="hero-art" aria-hidden="true">
        <div class="orb orb-one"></div><div class="orb orb-two"></div><div class="orbit orbit-one"></div><div class="orbit orbit-two"></div>
        <div class="plane-mark"><svg viewBox="0 0 48 48" fill="none"><path d="m8 23.5 30-12-8.7 25.2-6.3-10.5L8 23.5Z" fill="currentColor" opacity=".98"/><path d="m23 26.2 6.1 10.5 2.1-14.4L38 11.5 23 26.2Z" fill="currentColor" opacity=".56"/></svg></div>
      </div>
    </article>

    <div class="account-grid">
      <article class="setup-card">
        <div class="section-heading"><div><span class="eyebrow">Account setup</span><h3>One clear path to ready</h3></div><span class="completion-count">{configuredSteps}/3 configured</span></div>
        <p class="section-description">Complete the connection in a few focused steps. Your changes stay together, so you always know what will be saved.</p>
        <ol class="setup-preview">
          <li class:complete={Boolean(telegramApiId.trim())}><span class="step-number">1</span><span><strong>API access</strong><small>Identify your Telegram application</small></span><span class="step-state">{telegramApiId.trim() ? 'Ready' : 'Next'}</span></li>
          <li class:complete={Boolean(telegramStorageChatId.trim())}><span class="step-number">2</span><span><strong>Storage destination</strong><small>Choose the Telegram chat for objects</small></span><span class="step-state">{telegramStorageChatId.trim() ? 'Ready' : 'Next'}</span></li>
          <li class:complete={telegramProxyMode !== 'disabled'}><span class="step-number">3</span><span><strong>Network route</strong><small>Make the connection reliable</small></span><span class="step-state">{telegramProxyMode === 'disabled' ? 'Direct' : 'Auto'}</span></li>
          <li class:complete={connected}><span class="step-number">4</span><span><strong>Telegram sign-in</strong><small>Authorize the storage account</small></span><span class="step-state">{connected ? 'Ready' : 'Pending'}</span></li>
        </ol>
        <button class="primary wide-button" type="button" on:click={() => onToggleWizard(true)}>{connected ? 'Review account setup' : 'Start account setup'}<span aria-hidden="true">→</span></button>
      </article>

      <article class="summary-card">
        <div class="section-heading"><div><span class="eyebrow">Configuration at a glance</span><h3>Everything in one place</h3></div><span class="summary-icon" aria-hidden="true">✦</span></div>
        <div class="summary-list">
          <div class="summary-row"><span class="summary-icon-small">⌁</span><span><small>API application</small><strong>{maskedApiId}</strong></span></div>
          <div class="summary-row"><span class="summary-icon-small">⌂</span><span><small>Storage chat</small><strong>{maskedChatId}</strong></span></div>
          <div class="summary-row"><span class="summary-icon-small">↗</span><span><small>Network route</small><strong>{networkLabel}</strong></span></div>
          <div class="summary-row"><span class:summary-ok={connected} class="summary-icon-small">✓</span><span><small>Health status</small><strong>{connected ? 'Storage chat reachable' : statusLabel}</strong></span></div>
        </div>
        <div class="privacy-note"><span aria-hidden="true">●</span><span>Secrets stay protected and are never shown in this summary.</span></div>
      </article>
    </div>

    {#if settingsMessage || settingsError}
      <div class:message-error={Boolean(settingsError)} class="account-message" role={settingsError ? 'alert' : 'status'}>{settingsError || settingsMessage}</div>
    {/if}

    {#if hasConnection}
      <article class="danger-zone">
        <div><span class="eyebrow">Advanced action</span><h3>Remove this Telegram connection</h3><p>This hides the connection from the console and optionally schedules remote file cleanup.</p></div>
        <button class="danger-button" type="button" on:click={openRemoval}>Remove connection</button>
      </article>
    {/if}
  </section>
{/if}

{#if showRemoveConnection}
  <div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && !removeBusy && (showRemoveConnection = false)}>
    <form class="modal-card compact-modal" on:submit|preventDefault={removeConnection}>
      <div class="section-head"><div><p class="card-label">Telegram connection</p><h2>Remove current connection?</h2></div><button class="icon-button" type="button" aria-label="Close" on:click={() => showRemoveConnection = false} disabled={removeBusy}>×</button></div>
      <p class="fine-print">The dashboard will remove its buckets, files, and statistics immediately. This cannot be undone from this installation.</p>
      <label><span>Confirm the linked phone number</span><input type="tel" bind:value={phoneConfirmation} autocomplete="tel" placeholder="e.g. +1 555 123 4567" /></label>
      <p class="fine-print">Enter the phone number used for this Telegram connection. Formatting spaces, dashes, and parentheses are ignored.</p>
      <label class="checkbox-row"><input type="checkbox" bind:checked={deleteUploadedFiles} /><span>Also delete all uploaded Telegram files</span></label>
      <p class="fine-print">If unchecked, uploaded Telegram files remain in Telegram but this connection will no longer manage them. If checked, deletion runs in the background and may take time.</p>
      {#if removeError}<p class="error-hint" role="alert">{removeError}</p>{/if}
      <div class="settings-actions"><button class="ghost" type="button" on:click={() => showRemoveConnection = false} disabled={removeBusy}>Cancel</button><button class="danger-button" type="submit" disabled={removeBusy || !phoneConfirmation.trim()}>{removeBusy ? 'Removing…' : 'Remove connection'}</button></div>
    </form>
  </div>
{/if}

<style>
  .settings-tabs{display:flex;gap:10px;margin-bottom:18px;padding:6px;border:1px solid #d7e2ee;border-radius:18px;background:#f5f8fc;box-shadow:0 8px 24px rgba(23,43,77,.05)}
  .settings-tabs button{display:flex;align-items:center;gap:11px;flex:0 1 250px;padding:11px 14px;border:1px solid transparent;border-radius:13px;background:transparent;color:#5d718b;text-align:left}
  .settings-tabs button.active{border-color:#cbdced;background:#fff;color:#17345a;box-shadow:0 5px 14px rgba(23,58,96,.08)}
  .settings-tabs button:hover:not(:disabled){background:#fff;color:#17345a}
  .settings-tabs strong,.settings-tabs small{display:block}.settings-tabs strong{font-size:.86rem}.settings-tabs small{margin-top:2px;color:#8191a5;font-size:.72rem}
  .tab-icon{display:grid;place-items:center;width:32px;height:32px;border-radius:10px;background:#eaf3ff;color:#2874b7;font-size:1.1rem;font-weight:800}.storage-tab-icon{background:#e8f7f1;color:#1f8b69}
  .storage-policy-page{display:grid;gap:18px}.storage-hero{position:relative;overflow:hidden;display:grid;grid-template-columns:minmax(0,1.2fr) minmax(240px,.8fr);min-height:270px;padding:34px;border:1px solid #cce1ed;border-radius:28px;background:linear-gradient(118deg,#f8fcff 0%,#e9f7f8 54%,#dceefe 100%);box-shadow:0 20px 50px rgba(43,116,166,.12)}
  .storage-hero-copy{position:relative;z-index:1;max-width:650px}.storage-hero h2{max-width:650px;margin:20px 0 12px;color:#17345a;font-size:clamp(2rem,4vw,3.35rem);line-height:.98;letter-spacing:-.055em}.storage-hero p{max-width:590px;margin:0;color:#56708e;font-size:1rem;line-height:1.6}.live-pill{display:inline-flex;align-items:center;gap:7px;padding:6px 10px;border:1px solid #a8dccb;border-radius:999px;background:#effbf6;color:#21775c;font-size:.72rem;font-weight:800}.live-dot{width:7px;height:7px;border-radius:50%;background:#2fa87d;box-shadow:0 0 0 4px rgba(47,168,125,.12)}
  .policy-orbit{position:relative;min-height:200px}.policy-core{position:absolute;z-index:2;right:16%;top:50%;display:grid;place-items:center;width:144px;height:144px;border:1px solid rgba(255,255,255,.7);border-radius:50%;background:linear-gradient(145deg,#247ebc,#4d67c9);color:#fff;font-size:1.05rem;font-weight:850;box-shadow:0 20px 40px rgba(36,92,180,.25);transform:translateY(-50%)}.policy-ring{position:absolute;border:1px solid rgba(48,116,176,.25);border-radius:50%;transform:rotate(-23deg)}.ring-a{width:290px;height:120px;right:-1%;top:23%}.ring-b{width:235px;height:92px;right:7%;top:31%;border-color:rgba(44,155,158,.23);transform:rotate(28deg)}.policy-spark{position:absolute;width:12px;height:12px;border-radius:50%;background:#42b9ac;box-shadow:0 0 0 7px rgba(66,185,172,.12)}.spark-a{right:12%;top:12%}.spark-b{right:42%;bottom:10%;width:8px;height:8px;background:#6780d0}
  .policy-grid{display:grid;grid-template-columns:minmax(0,1.12fr) minmax(300px,.88fr);gap:18px}.policy-card{border:1px solid var(--border);border-radius:22px;background:var(--surface);box-shadow:0 10px 30px rgba(23,43,77,.05)}.policy-editor,.policy-impact{padding:24px}.policy-card-heading{display:flex;align-items:flex-start;justify-content:space-between;gap:14px}.policy-card-heading h3{margin:6px 0 0;color:#17345a;font-size:1.35rem;letter-spacing:-.035em}.policy-badge{padding:7px 10px;border-radius:999px;background:#eaf3ff;color:#28679d;font-size:.73rem;font-weight:800;white-space:nowrap}.policy-description{margin:15px 0 22px;color:var(--muted);line-height:1.55}.chunk-input-label{display:grid;gap:8px;color:#365370;font-weight:800}.chunk-input-wrap{display:flex;align-items:center;overflow:hidden;border:1px solid #cbd9e7;border-radius:14px;background:#fbfdff;box-shadow:inset 0 1px 2px rgba(26,57,89,.04)}.chunk-input-wrap:focus-within{border-color:#4c9bd0;box-shadow:0 0 0 4px rgba(76,155,208,.12)}.chunk-input-wrap input{min-width:0;flex:1;padding:14px 15px;border:0;background:transparent;color:#17345a;font-size:1.35rem;font-weight:850;outline:0}.chunk-input-wrap span{padding:0 16px;color:#6b829a;font-weight:800}.preset-grid{display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:8px;margin-top:12px}.preset-grid button{padding:10px 5px;border:1px solid #d7e2ec;border-radius:11px;background:#fff;color:#5c728c;font-size:.76rem;font-weight:800}.preset-grid button:hover:not(:disabled),.preset-grid button.chosen{border-color:#80b8db;background:#edf7ff;color:#1e6ea8}.range-help{margin:14px 0 0;color:#8493a3;font-size:.74rem;line-height:1.5}.range-help strong{color:#52718d}.policy-actions{display:flex;align-items:center;justify-content:space-between;gap:12px;margin-top:22px}.policy-actions .primary{white-space:nowrap}.draft-preview{color:#a25a25;font-size:.76rem;font-weight:800}.draft-preview.valid{color:#198061}.storage-message{margin:16px 0 0;padding:11px 13px;border:1px solid #b8ddcf;border-radius:12px;background:#effbf5;color:#23775c;font-size:.78rem;line-height:1.45}.storage-message.message-error{border-color:#efc2c2;background:#fff5f5;color:var(--danger)}.shield-mark{display:grid;place-items:center;width:32px;height:32px;border-radius:11px;background:#e1f6ed;color:#16815b;font-weight:900}.impact-list{display:grid;gap:5px;margin-top:23px}.impact-row{display:flex;align-items:flex-start;gap:12px;padding:13px 0;border-bottom:1px solid #edf1f5}.impact-row:last-child{border-bottom:0}.impact-row strong,.impact-row small{display:block}.impact-row strong{font-size:.86rem;color:#294967}.impact-row small{margin-top:4px;color:var(--muted);font-size:.75rem;line-height:1.45}.impact-icon{display:grid;place-items:center;flex:0 0 auto;width:30px;height:30px;border-radius:10px;font-weight:900}.impact-icon.blue{background:#eaf3ff;color:#2874b7}.impact-icon.green{background:#e8f7f1;color:#1f8b69}.impact-icon.amber{background:#fff3d6;color:#bd791e}.policy-note{display:flex;align-items:flex-start;gap:9px;margin-top:18px;padding:12px;border-radius:13px;background:#f7fafc;color:#6f8194;font-size:.73rem;line-height:1.5}.policy-note>span:first-child{display:grid;place-items:center;flex:0 0 auto;width:18px;height:18px;border-radius:50%;background:#dbeaf5;color:#46779a;font-weight:900}.policy-note code{padding:2px 4px;border-radius:4px;background:#eef3f7;color:#425e76;font-size:.7rem}
  .telegram-account-page{display:grid;gap:18px}
  .account-hero{position:relative;overflow:hidden;display:grid;grid-template-columns:minmax(0,1.45fr) minmax(220px,.55fr);min-height:290px;padding:34px;border:1px solid #c9d9ef;border-radius:28px;background:linear-gradient(120deg,#f9fcff 0%,#eaf3ff 54%,#dbeeff 100%);box-shadow:0 20px 50px rgba(45,104,166,.12)}
  .hero-copy-block{position:relative;z-index:1;max-width:650px}.eyebrow-row,.hero-actions,.section-heading,.summary-row,.privacy-note,.danger-zone{display:flex;align-items:center}.eyebrow-row{gap:12px}.eyebrow{font-size:.7rem;font-weight:800;letter-spacing:.16em;text-transform:uppercase;color:#5b7290}.status-pill{display:inline-flex;align-items:center;gap:7px;padding:6px 10px;border:1px solid #e1a76e;border-radius:999px;background:#fff7ed;color:#97511b;font-size:.74rem;font-weight:800}.status-pill.connected{border-color:#9ed8c5;background:#effbf5;color:#1e7757}.status-dot{width:7px;height:7px;border-radius:50%;background:currentColor}.account-hero h2{max-width:620px;margin:20px 0 12px;font-size:clamp(2rem,4vw,3.5rem);line-height:.98;letter-spacing:-.055em;color:#17345a}.account-hero p{max-width:570px;margin:0;color:#56708e;font-size:1.02rem;line-height:1.6}.hero-actions{gap:18px;margin-top:28px}.hero-button,.wide-button{gap:12px;font-weight:800}.hero-button{padding:13px 17px;border-radius:12px;box-shadow:0 10px 22px rgba(33,109,186,.2)}.hero-link{padding:0;background:transparent;color:#32658f;font-weight:800}.hero-link:hover:not(:disabled){background:transparent;color:var(--accent);transform:none}.hero-art{position:relative;min-height:220px}.orb,.orbit{position:absolute;border-radius:50%}.orb-one{width:160px;height:160px;right:18%;top:22%;background:linear-gradient(135deg,#2f91d5,#4e62c1);box-shadow:0 20px 40px rgba(43,91,177,.22)}.orb-two{width:72px;height:72px;right:4%;bottom:10%;background:linear-gradient(135deg,#4ec6bc,#277aa7);opacity:.82}.orbit-one{width:270px;height:122px;right:-4%;top:30%;border:1px solid rgba(47,102,177,.23);transform:rotate(-28deg)}.orbit-two{width:220px;height:100px;right:7%;top:38%;border:1px solid rgba(47,102,177,.17);transform:rotate(25deg)}.plane-mark{position:absolute;right:27%;top:42%;display:grid;place-items:center;width:70px;height:70px;border:1px solid rgba(255,255,255,.35);border-radius:22px;background:rgba(255,255,255,.2);color:#fff;backdrop-filter:blur(10px);transform:rotate(-10deg);box-shadow:0 10px 30px rgba(35,83,165,.18)}.plane-mark svg{width:42px}.account-grid{display:grid;grid-template-columns:1.1fr .9fr;gap:18px}.setup-card,.summary-card,.danger-zone{border:1px solid var(--border);border-radius:22px;background:var(--surface);box-shadow:0 10px 30px rgba(23,43,77,.05)}.setup-card,.summary-card{padding:24px}.section-heading{justify-content:space-between;gap:12px}.section-heading h3,.danger-zone h3{margin:6px 0 0;font-size:1.28rem;letter-spacing:-.03em}.completion-count{padding:7px 10px;border-radius:999px;background:var(--accent-soft);color:var(--accent);font-size:.76rem;font-weight:800}.section-description{margin:14px 0 18px;color:var(--muted);line-height:1.55}.setup-preview{display:grid;gap:8px;margin:0 0 20px;padding:0;list-style:none}.setup-preview li{display:grid;grid-template-columns:32px minmax(0,1fr) auto;align-items:center;gap:11px;padding:11px 12px;border:1px solid #e7edf4;border-radius:14px;background:#fbfcfe}.setup-preview li.complete{border-color:#c5e7d7;background:#f5fcf8}.step-number{display:grid;place-items:center;width:29px;height:29px;border-radius:10px;background:#e7eff8;color:#5b7290;font-size:.78rem;font-weight:900}.complete .step-number{background:#d6f1e2;color:#16815b}.setup-preview strong,.summary-row strong{display:block;font-size:.88rem}.setup-preview small,.summary-row small{display:block;margin-top:2px;color:var(--muted);font-size:.73rem}.step-state{color:#7c8da0;font-size:.7rem;font-weight:800;text-transform:uppercase;letter-spacing:.08em}.complete .step-state{color:var(--ok)}.wide-button{width:100%;padding:13px}.summary-icon{display:grid;place-items:center;width:30px;height:30px;border-radius:10px;background:#fff3d6;color:#c7831c}.summary-list{display:grid;gap:2px;margin-top:24px}.summary-row{gap:12px;padding:12px 0;border-bottom:1px solid #edf1f5}.summary-row:last-child{border-bottom:0}.summary-icon-small{display:grid;place-items:center;flex:0 0 auto;width:28px;height:28px;border-radius:9px;background:#eef5fb;color:#44739b;font-weight:800}.summary-icon-small.summary-ok{background:#e0f5e8;color:var(--ok)}.privacy-note{gap:8px;margin-top:18px;padding:12px;border-radius:12px;background:#f7fafc;color:var(--muted);font-size:.74rem;line-height:1.45}.privacy-note>span:first-child{color:#52a387;font-size:.6rem}.account-message{padding:13px 16px;border:1px solid #b8ddcf;border-radius:14px;background:#effbf5;color:#23775c;font-size:.86rem}.account-message.message-error{border-color:#efc2c2;background:#fff5f5;color:var(--danger)}.danger-zone{justify-content:space-between;gap:18px;align-items:center;padding:20px 24px}.danger-zone p{margin:8px 0 0;color:var(--muted);font-size:.84rem}.danger-button{background:var(--danger);color:#fff;border-color:var(--danger)}.danger-button:hover:not(:disabled){background:#8f2929}.checkbox-row{display:flex;align-items:flex-start;gap:10px;font-weight:700}.checkbox-row input{width:18px;height:18px;accent-color:var(--danger)}.modal-backdrop{position:fixed;inset:0;background:rgba(12,25,42,.58);display:grid;place-items:center;padding:20px;z-index:30}.modal-card{width:min(620px,100%);max-height:calc(100vh - 40px);overflow:auto;display:grid;gap:16px;padding:22px;border:1px solid var(--border);border-radius:var(--radius-lg);background:var(--surface);box-shadow:0 20px 60px rgba(13,31,52,.25)}.compact-modal{width:min(430px,100%)}.icon-button{width:36px;height:36px;padding:0;border-radius:50%;background:var(--accent-soft);color:var(--text);font-size:1.35rem}.settings-actions{display:flex;justify-content:flex-end;gap:10px;margin-top:1rem}.error-hint{color:var(--danger)}
  @media(max-width:820px){.account-hero{grid-template-columns:1fr;min-height:0;padding:26px}.hero-art{position:absolute;right:-42px;top:30px;width:270px;opacity:.38;pointer-events:none}.account-hero h2,.account-hero p{max-width:72%}.account-grid{grid-template-columns:1fr}}
  @media(max-width:820px){.storage-hero{grid-template-columns:1fr;min-height:0;padding:26px}.policy-orbit{position:absolute;right:-60px;top:30px;width:270px;opacity:.35;pointer-events:none}.storage-hero h2,.storage-hero p{max-width:72%}.policy-grid{grid-template-columns:1fr}}
  @media(max-width:560px){.settings-tabs{gap:5px}.settings-tabs button{flex:1;padding:9px 8px;gap:7px}.settings-tabs small{display:none}.tab-icon{width:28px;height:28px}.account-hero,.storage-hero{padding:22px}.account-hero h2,.storage-hero h2{max-width:100%;font-size:2.2rem}.account-hero p,.storage-hero p{max-width:100%;font-size:.92rem}.hero-actions{align-items:flex-start;flex-direction:column;gap:14px}.setup-card,.summary-card,.policy-editor,.policy-impact{padding:18px}.danger-zone{align-items:flex-start;flex-direction:column;padding:18px}.danger-zone .danger-button{width:100%}.setup-preview li{grid-template-columns:29px minmax(0,1fr);gap:9px}.step-state{grid-column:2}.modal-backdrop{padding:10px}.preset-grid{grid-template-columns:repeat(3,minmax(0,1fr))}.policy-actions{align-items:stretch;flex-direction:column}.policy-actions .primary{width:100%}}
</style>
