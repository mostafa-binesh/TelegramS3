<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { wizardBegin, wizardCancel, wizardSubmitCode, wizardSubmitPassword } from '../lib/api';
  import type { WizardPhase, WizardState } from '../lib/types';

  export let csrf: string | null | undefined;
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
  export let onSave: () => Promise<void> | void = () => {};
  export let onDone: () => void = () => {};
  export let onClose: () => void = () => {};

  const steps = [
    { number: 1, title: 'API access', detail: 'Application credentials', icon: '⌁' },
    { number: 2, title: 'Storage chat', detail: 'Object destination', icon: '⌂' },
    { number: 3, title: 'Network route', detail: 'Proxy and reachability', icon: '↗' },
    { number: 4, title: 'Sign in', detail: 'Authorize Telegram', icon: '✓' }
  ];

  let step = 1;
  let furthestStep = 1;
  let pending = false;
  let phase: WizardPhase = 'idle';
  let inlineError = '';
  let phone = '';
  let code = '';
  let password = '';
  let flowId = newFlowId();
  let apiIdInput: HTMLInputElement;
  let apiHashInput: HTMLInputElement;
  let storageChatInput: HTMLInputElement;
  let phoneInput: HTMLInputElement;
  let codeInput: HTMLInputElement;
  let passwordInput: HTMLInputElement;

  $: hasApiCredentials = Boolean(telegramApiId.trim() && telegramApiHash.trim());
  $: hasStorageChat = Boolean(telegramStorageChatId.trim());
  $: networkReady = telegramProxyMode === 'disabled' || telegramProxyMode === 'auto' || Boolean(telegramProxyUrl.trim());
  $: canContinue = step === 1 ? hasApiCredentials : step === 2 ? hasStorageChat : step === 3 ? networkReady : false;
  $: visibleError = inlineError || settingsError;

  function newFlowId() {
    return globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }

  onMount(() => {
    void tick().then(() => apiIdInput?.focus());
  });

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && !pending) {
      event.preventDefault();
      void close();
    }
  }

  function fail(cause: unknown) {
    inlineError = cause instanceof Error ? cause.message : 'Something went wrong';
  }

  function validateCurrentStep() {
    inlineError = '';
    if (step === 1 && !hasApiCredentials) {
      inlineError = 'Enter both the Telegram API ID and API hash to continue.';
      return false;
    }
    if (step === 2 && !hasStorageChat) {
      inlineError = 'Enter the Telegram chat ID that should store your objects.';
      return false;
    }
    if (step === 3 && telegramProxyMode !== 'disabled' && !networkReady) {
      inlineError = 'Enter a proxy URL or choose Direct connection.';
      return false;
    }
    return true;
  }

  async function focusStep() {
    await tick();
    if (step === 1) apiIdInput?.focus();
    if (step === 2) storageChatInput?.focus();
    if (step === 4 && phase === 'idle') phoneInput?.focus();
  }

  async function nextStep() {
    if (pending || phase !== 'idle' || !validateCurrentStep()) return;
    if (step < 4) {
      step += 1;
      furthestStep = Math.max(furthestStep, step);
      await focusStep();
    }
  }

  async function previousStep() {
    if (pending || phase !== 'idle' || step <= 1) return;
    inlineError = '';
    step -= 1;
    await focusStep();
  }

  async function goToStep(target: number) {
    if (pending || phase !== 'idle' || target < 1 || target > furthestStep) return;
    if (target > step && !validateCurrentStep()) return;
    step = target;
    inlineError = '';
    await focusStep();
  }

  function handToPhase(state: WizardState) {
    phase = state.phase;
    inlineError = state.phase === 'authorized' && state.connection_ready === false
      ? (state.health_detail || state.message || 'Telegram account authorized, but storage is not ready.')
      : '';
    if (state.phase === 'two_fa') code = '';
    void tick().then(() => {
      if (phase === 'code') codeInput?.focus();
      if (phase === 'two_fa') passwordInput?.focus();
    });
    if (state.phase === 'authorized' && state.connection_ready !== false) {
      onDone();
    }
  }

  async function startLogin() {
    if (pending || !phone.trim()) return;
    pending = true;
    inlineError = '';
    try {
      await onSave();
      handToPhase(await wizardBegin(phone.trim(), flowId, csrf));
    } catch (cause) {
      fail(cause);
    } finally {
      pending = false;
    }
  }

  async function submitCode() {
    if (pending || !code.trim()) return;
    pending = true;
    inlineError = '';
    try {
      handToPhase(await wizardSubmitCode(code.trim(), flowId, csrf));
    } catch (cause) {
      fail(cause);
    } finally {
      pending = false;
    }
  }

  async function submitPassword() {
    if (pending || !password.trim()) return;
    pending = true;
    inlineError = '';
    try {
      handToPhase(await wizardSubmitPassword(password, flowId, csrf));
    } catch (cause) {
      fail(cause);
    } finally {
      pending = false;
    }
  }

  async function restartLogin() {
    if (pending) return;
    pending = true;
    inlineError = '';
    try {
      await wizardCancel(flowId, csrf);
      flowId = newFlowId();
      phase = 'idle';
      code = '';
      password = '';
      await tick();
      phoneInput?.focus();
    } catch (cause) {
      fail(cause);
    } finally {
      pending = false;
    }
  }

  async function close() {
    if (pending) return;
    pending = true;
    try {
      await wizardCancel(flowId, csrf);
    } catch {
      // Closing the local wizard still wins if the server flow has already ended.
    } finally {
      pending = false;
      onClose();
    }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<section class="wizard-page" aria-labelledby="telegram-wizard-title">
  <div class="wizard-topbar">
    <div><span class="eyebrow">Telegram account setup</span><h2 id="telegram-wizard-title">Connect your storage account</h2><p>Four focused steps. One dependable connection.</p></div>
    <button class="close-wizard" type="button" on:click={() => void close()} disabled={pending} aria-label="Close account setup">× <span>Back to account</span></button>
  </div>

  <div class="wizard-layout">
    <aside class="wizard-rail" aria-label="Account setup steps">
      <div class="rail-intro"><span class="rail-badge">{step}<small>/4</small></span><div><span class="eyebrow">Current step</span><strong>{steps[step - 1].title}</strong></div></div>
      <ol>
        {#each steps as item}
          <li class:active={step === item.number} class:complete={item.number < step || (item.number === 4 && phase === 'authorized')}>
            <button type="button" on:click={() => void goToStep(item.number)} disabled={item.number > furthestStep || pending || phase !== 'idle'} aria-current={step === item.number ? 'step' : undefined}>
              <span class="rail-number">{item.number < step ? '✓' : item.number}</span><span><strong>{item.title}</strong><small>{item.detail}</small></span>
            </button>
          </li>
        {/each}
      </ol>
      <div class="rail-note"><span aria-hidden="true">✦</span><p>Your settings are saved together before Telegram sign-in begins.</p></div>
    </aside>

    <div class="wizard-content">
      <div class="progress-header"><span>Step {step} of 4</span><span>{Math.round((step / 4) * 100)}%</span></div><div class="progress-track"><span style={`width:${(step / 4) * 100}%`}></span></div>

      {#if step === 1}
        <div class="wizard-step-card">
          <div class="step-heading"><span class="large-step-number">01</span><div><span class="eyebrow">API credentials</span><h3>Start with your Telegram app</h3><p>These credentials identify the application that will access your storage account.</p></div></div>
          <div class="form-grid">
            <label><span>Telegram API ID</span><input bind:this={apiIdInput} bind:value={telegramApiId} inputmode="numeric" autocomplete="off" placeholder="12345678" aria-describedby="api-id-help" /><small id="api-id-help">A numeric ID from my.telegram.org.</small></label>
            <label><span>Telegram API hash</span><input bind:this={apiHashInput} bind:value={telegramApiHash} type="password" autocomplete="off" placeholder="Paste your API hash" /><small>Keep this value private. It is never shown in the summary.</small></label>
          </div>
          <div class="info-callout"><span>i</span><p>Use the API ID and hash from the Telegram application you created for this storage service.</p></div>
        </div>
      {:else if step === 2}
        <div class="wizard-step-card">
          <div class="step-heading"><span class="large-step-number">02</span><div><span class="eyebrow">Storage destination</span><h3>Choose where objects live</h3><p>Telegram documents are stored in one dedicated chat. Use its signed chat ID so the store can find it reliably.</p></div></div>
          <label class="single-field"><span>Storage chat ID</span><input bind:this={storageChatInput} bind:value={telegramStorageChatId} inputmode="numeric" autocomplete="off" placeholder="-1001234567890" aria-describedby="chat-id-help" /><small id="chat-id-help">Usually starts with <code>-100</code> for a supergroup or channel.</small></label>
          <div class="destination-visual"><div class="destination-icon">⌂</div><div><strong>One durable destination</strong><p>Manifests and chunks will be organized here. The local metadata index remains the fast lookup layer.</p></div><span class="destination-check">✓</span></div>
        </div>
      {:else if step === 3}
        <div class="wizard-step-card">
          <div class="step-heading"><span class="large-step-number">03</span><div><span class="eyebrow">Network route</span><h3>Make the connection reliable</h3><p>Choose how this server reaches Telegram. Automatic routing is a good default.</p></div></div>
          <label class="single-field"><span>Connection mode</span><select bind:value={telegramProxyMode}><option value="auto">Automatic routing</option><option value="disabled">Direct connection</option><option value="socks5">SOCKS5 proxy</option><option value="http">HTTP proxy</option></select></label>
          {#if telegramProxyMode !== 'disabled'}
            <label class="single-field"><span>Proxy URL</span><input bind:value={telegramProxyUrl} placeholder="socks5://127.0.0.1:12334" autocomplete="url" /><small>Include the protocol and port when your network requires a proxy.</small></label>
            <div class="form-grid"><label><span>Proxy username <em>Optional</em></span><input bind:value={telegramProxyUsername} autocomplete="username" /></label><label><span>Proxy password <em>Optional</em></span><input bind:value={telegramProxyPassword} type="password" autocomplete="current-password" /></label></div>
          {/if}
          <div class="network-preview"><span class="pulse-dot"></span><span><strong>{telegramProxyMode === 'disabled' ? 'Direct connection selected' : 'Route ready to test'}</strong><small>{telegramProxyMode === 'disabled' ? 'The server will connect without a proxy.' : 'The connection will use your selected proxy settings.'}</small></span></div>
        </div>
      {:else}
        <div class="wizard-step-card auth-card">
          <div class="step-heading"><span class="large-step-number">04</span><div><span class="eyebrow">Telegram sign-in</span><h3>{phase === 'idle' ? 'Authorize the storage account' : phase === 'code' ? 'Enter the confirmation code' : phase === 'two_fa' ? 'Confirm your cloud password' : 'Checking your connection'}</h3><p>{phase === 'idle' ? 'Your settings are ready. Telegram will send a code to the phone number below.' : phase === 'code' ? 'Telegram sent a confirmation code to the account above.' : phase === 'two_fa' ? 'This account has two-step verification enabled.' : 'Telegram accepted the sign-in. Verifying the storage chat now.'}</p></div></div>
          {#if phase === 'idle'}
            <form class="auth-form" on:submit|preventDefault={() => void startLogin()}>
              <label class="single-field"><span>Phone number</span><input bind:this={phoneInput} bind:value={phone} type="tel" inputmode="tel" autocomplete="tel" placeholder="+1 555 000 0000" /><small>Use international format. This is used only for the current sign-in.</small></label>
              <div class="save-ready"><span>✓</span><div><strong>Settings ready to save</strong><small>API access, storage chat, and network route will be saved together.</small></div></div>
              <button class="primary auth-button" type="submit" disabled={pending || settingsBusy || !phone.trim()}>{pending || settingsBusy ? 'Saving and sending…' : 'Save settings & send code'}<span aria-hidden="true">→</span></button>
            </form>
          {:else if phase === 'code'}
            <form class="auth-form" on:submit|preventDefault={() => void submitCode()}>
              <label class="single-field"><span>Confirmation code</span><input bind:this={codeInput} bind:value={code} type="text" inputmode="numeric" autocomplete="one-time-code" placeholder="6-digit code" maxlength="6" /></label>
              <button class="primary auth-button" type="submit" disabled={pending || !code.trim()}>{pending ? 'Checking…' : 'Confirm code'}<span aria-hidden="true">→</span></button>
              <button class="text-button" type="button" on:click={() => void restartLogin()} disabled={pending}>Send a new code</button>
            </form>
          {:else if phase === 'two_fa'}
            <form class="auth-form" on:submit|preventDefault={() => void submitPassword()}>
              <label class="single-field"><span>Cloud password</span><input bind:this={passwordInput} bind:value={password} type="password" autocomplete="current-password" placeholder="Enter your Telegram cloud password" /></label>
              <button class="primary auth-button" type="submit" disabled={pending || !password.trim()}>{pending ? 'Authorizing…' : 'Authorize account'}<span aria-hidden="true">→</span></button>
            </form>
          {:else}
            <div class="success-panel"><div class="success-mark">✓</div><div><strong>Telegram account authorized</strong><p>We are checking that the storage chat is reachable before returning you to the account overview.</p></div></div>
          {/if}
        </div>
      {/if}

      {#if pending}<p class="working" aria-live="polite"><span class="spinner"></span> Working securely…</p>{/if}
      {#if visibleError}<p class="wizard-error" role="alert">{visibleError}</p>{/if}
      {#if settingsMessage && !visibleError}<p class="wizard-success" role="status">{settingsMessage}</p>{/if}

      <footer class="wizard-footer">
        {#if phase === 'idle'}
          <button class="ghost" type="button" on:click={() => void previousStep()} disabled={pending || step <= 1}>← Back</button>
          {#if step < 4}<button class="primary next-button" type="button" on:click={() => void nextStep()} disabled={pending || !canContinue}>{step === 3 ? 'Review & sign in' : 'Continue'}<span aria-hidden="true">→</span></button>{/if}
        {:else}
          <button class="ghost" type="button" on:click={() => void restartLogin()} disabled={pending}>Restart sign-in</button>
        {/if}
      </footer>
    </div>
  </div>
</section>

<style>
  .wizard-page{display:grid;gap:22px;padding:28px;border:1px solid #c8d8eb;border-radius:28px;background:linear-gradient(145deg,#f8fbff 0%,#eef5fc 48%,#f9fbfd 100%);box-shadow:0 20px 50px rgba(31,83,137,.1)}.wizard-topbar{display:flex;justify-content:space-between;gap:18px;align-items:flex-start}.wizard-topbar h2{margin:8px 0 5px;font-size:clamp(1.7rem,3vw,2.6rem);letter-spacing:-.05em;color:#17345a}.wizard-topbar p{margin:0;color:#627992}.close-wizard{padding:8px 11px;background:rgba(255,255,255,.75);border:1px solid #d6e2ef;color:#4d6681;font-weight:700}.close-wizard span{font-size:.78rem}.wizard-layout{display:grid;grid-template-columns:250px minmax(0,1fr);gap:24px}.wizard-rail{padding:20px;border-radius:22px;background:linear-gradient(160deg,#19385f,#22527d);color:#f3f8ff;box-shadow:0 16px 30px rgba(24,58,97,.16)}.rail-intro{display:flex;align-items:center;gap:12px;padding-bottom:22px;border-bottom:1px solid rgba(255,255,255,.15)}.rail-badge{display:grid;place-items:center;width:47px;height:47px;border-radius:15px;background:#6bc4bf;color:#123a5c;font-size:1.35rem;font-weight:900}.rail-badge small{font-size:.6rem;opacity:.7}.rail-intro .eyebrow{display:block;color:#a7c5e2;font-size:.6rem}.rail-intro strong{display:block;margin-top:5px;font-size:.94rem}.wizard-rail ol{display:grid;gap:5px;margin:20px 0;padding:0;list-style:none}.wizard-rail li button{display:grid;grid-template-columns:32px 1fr;align-items:center;gap:10px;width:100%;padding:11px 8px;background:transparent;color:#afc7de;text-align:left;border-radius:13px}.wizard-rail li button:hover:not(:disabled){background:rgba(255,255,255,.1);transform:none}.wizard-rail li.active button{background:rgba(255,255,255,.13);color:#fff}.wizard-rail li.complete button{color:#c3efd8}.rail-number{display:grid;place-items:center;width:27px;height:27px;border:1px solid rgba(255,255,255,.24);border-radius:9px;font-size:.76rem;font-weight:900}.active .rail-number{border-color:#72d0ca;background:#72d0ca;color:#153c5d}.complete .rail-number{border-color:#9addbd;background:#9addbd;color:#164e3c}.wizard-rail li strong{display:block;font-size:.8rem}.wizard-rail li small{display:block;margin-top:3px;font-size:.68rem;opacity:.72}.rail-note{display:flex;gap:9px;padding:13px;border:1px solid rgba(255,255,255,.15);border-radius:14px;background:rgba(255,255,255,.08);color:#c8d9e9}.rail-note span{color:#82d7cb}.rail-note p{margin:0;font-size:.7rem;line-height:1.45}.wizard-content{min-width:0;padding:3px 4px}.progress-header{display:flex;justify-content:space-between;color:#6a829b;font-size:.73rem;font-weight:800;text-transform:uppercase;letter-spacing:.1em}.progress-track{height:5px;margin:9px 0 24px;border-radius:99px;background:#dbe6f0;overflow:hidden}.progress-track span{display:block;height:100%;border-radius:inherit;background:linear-gradient(90deg,#347bc0,#65c7be);transition:width .2s ease}.wizard-step-card{display:grid;gap:22px;min-height:415px;padding:28px;border:1px solid #d7e2ed;border-radius:22px;background:rgba(255,255,255,.88);box-shadow:0 10px 28px rgba(35,74,112,.06)}.step-heading{display:flex;align-items:flex-start;gap:16px}.large-step-number{display:grid;place-items:center;flex:0 0 auto;width:51px;height:51px;border-radius:16px;background:#e2effa;color:#3477b5;font-size:.94rem;font-weight:900;letter-spacing:.04em}.step-heading h3{margin:7px 0 7px;font-size:1.55rem;letter-spacing:-.045em;color:#203d5e}.step-heading p{max-width:58ch;margin:0;color:#6a7e93;line-height:1.55}.form-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}.single-field,.form-grid label{display:grid;gap:7px}.single-field>span,.form-grid label>span{font-size:.8rem;color:#405b76;font-weight:800}.single-field small,.form-grid small{color:#718499;font-size:.7rem;line-height:1.4}.form-grid em{font-style:normal;color:#8ea1b3;font-weight:500}.form-grid input,.single-field input,.single-field select{padding:13px 14px;border-radius:13px;border-color:#d6e2ec;background:#fbfdff}.info-callout,.destination-visual,.network-preview,.save-ready{display:flex;align-items:center;gap:11px;padding:14px 16px;border:1px solid #d5e7f3;border-radius:15px;background:#f3f9fd;color:#496983}.info-callout>span{display:grid;place-items:center;flex:0 0 auto;width:22px;height:22px;border-radius:50%;background:#d3ebf5;color:#2e78a1;font-size:.74rem;font-weight:900}.info-callout p,.destination-visual p{margin:0;font-size:.76rem;line-height:1.5}.destination-visual{margin-top:4px;background:linear-gradient(100deg,#f2fbfa,#f2f8ff);border-color:#c7e4df}.destination-icon{display:grid;place-items:center;width:42px;height:42px;border-radius:13px;background:#d8f1ea;color:#29856e;font-size:1.2rem}.destination-visual strong,.network-preview strong,.save-ready strong{display:block;color:#2f526c;font-size:.8rem}.destination-check{margin-left:auto;color:#1a9b70;font-size:1.1rem;font-weight:900}.network-preview{margin-top:2px;background:#f7fafc;border-color:#e1e9f0}.pulse-dot{width:10px;height:10px;border-radius:50%;background:#5ebd9b;box-shadow:0 0 0 5px #dff4e9}.network-preview small,.save-ready small{display:block;margin-top:3px;color:#7890a3;font-size:.7rem}.auth-card{min-height:390px}.auth-form{display:grid;align-content:start;gap:18px;max-width:640px}.save-ready{background:#f0fbf5;border-color:#c9ead8}.save-ready>span{display:grid;place-items:center;flex:0 0 auto;width:26px;height:26px;border-radius:9px;background:#d7f2e2;color:#15845c;font-weight:900}.auth-button{width:100%;justify-content:space-between;padding:14px 16px;border-radius:13px;font-weight:800}.text-button{justify-self:start;padding:0;background:transparent;color:#386d9b;font-weight:800}.text-button:hover:not(:disabled){background:transparent;color:var(--accent);transform:none}.success-panel{display:flex;align-items:center;gap:14px;padding:22px;border:1px solid #bde5cf;border-radius:16px;background:#f2fcf6}.success-mark{display:grid;place-items:center;width:46px;height:46px;border-radius:15px;background:#d3f2df;color:#18815b;font-size:1.35rem;font-weight:900}.success-panel strong{color:#216d53}.success-panel p{margin:6px 0 0;color:#638172;font-size:.8rem;line-height:1.5}.working,.wizard-error,.wizard-success{display:flex;align-items:center;gap:8px;margin:14px 0 0;font-size:.82rem}.working{color:#5f7890}.spinner{width:.9em;height:.9em;border:2px solid currentColor;border-top-color:transparent;border-radius:50%;animation:spin 700ms linear infinite}.wizard-error{padding:12px 14px;border:1px solid #efc7c7;border-radius:12px;background:#fff5f5;color:var(--danger)}.wizard-success{color:#24785d}.wizard-footer{display:flex;justify-content:space-between;gap:12px;margin-top:2px}.next-button{min-width:140px}.wizard-footer button{font-weight:800}@keyframes spin{to{transform:rotate(360deg)}}@media(prefers-reduced-motion:reduce){.progress-track span,.spinner{animation:none;transition:none}}@media(max-width:860px){.wizard-page{padding:20px}.wizard-layout{grid-template-columns:1fr}.wizard-rail{padding:16px}.wizard-rail ol{grid-template-columns:repeat(4,minmax(0,1fr));gap:6px}.wizard-rail li button{grid-template-columns:1fr;justify-items:center;padding:10px 5px;text-align:center}.rail-note{display:none}.rail-intro{padding-bottom:14px}.wizard-step-card{min-height:0}}
  @media(max-width:560px){.wizard-topbar{align-items:flex-start}.close-wizard span{display:none}.form-grid{grid-template-columns:1fr}.wizard-step-card{padding:20px}.step-heading{gap:11px}.large-step-number{width:42px;height:42px;border-radius:13px}.step-heading h3{font-size:1.3rem}.wizard-rail ol{overflow:hidden}.wizard-rail li strong{font-size:.64rem;line-height:1.12;letter-spacing:-.02em;overflow-wrap:anywhere}.wizard-rail li small{display:none}.wizard-rail li button{padding-right:1px;padding-left:1px}.wizard-rail .rail-number{width:24px;height:24px}.wizard-footer{position:sticky;bottom:0;padding-top:12px;background:linear-gradient(180deg,transparent,#f4f8fc 28%)} }
</style>
