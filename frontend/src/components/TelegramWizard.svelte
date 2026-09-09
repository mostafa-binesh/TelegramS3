<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { wizardBegin, wizardCancel, wizardSubmitCode, wizardSubmitPassword } from '../lib/api';
  import type { WizardPhase } from '../lib/types';

  export let csrf: string | null | undefined;
  export let onDone: () => void = () => {};
  export let onClose: () => void = () => {};

  let pending = false;
  let phase: WizardPhase = 'idle';
  let inlineError = '';
  let phone = '';
  let code = '';
  let password = '';
  let phoneInput: HTMLInputElement;
  let dialog: HTMLDivElement;
  let previouslyFocused: HTMLElement | null = null;
  let flowId = newFlowId();

  function newFlowId() {
    return globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }

  onMount(() => {
    previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    void tick().then(() => phoneInput?.focus());
    return () => previouslyFocused?.focus();
  });

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && !pending) { event.preventDefault(); void cancel(); }
    if (event.key === 'Tab' && dialog) {
      const focusable = [...dialog.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)')];
      if (!focusable.length) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    }
  }

  function restoreFocus() { previouslyFocused?.focus(); }

  function handToPhase(state: { phase: WizardPhase }) {
    phase = state.phase;
    inlineError = '';
    if (state.phase === 'two_fa') code = '';
    if (state.phase === 'authorized') { code = ''; password = ''; restoreFocus(); onDone(); }
  }

  function fail(cause: unknown) { inlineError = cause instanceof Error ? cause.message : 'Something went wrong'; }

  async function sendCode() {
    if (pending) return;
    pending = true; inlineError = '';
    try { handToPhase(await wizardBegin(phone.trim(), flowId, csrf)); }
    catch (cause) { fail(cause); }
    finally { pending = false; }
  }

  async function submitCode() {
    if (pending || !code.trim()) return;
    pending = true; inlineError = '';
    try { handToPhase(await wizardSubmitCode(code.trim(), flowId, csrf)); }
    catch (cause) { fail(cause); }
    finally { pending = false; }
  }

  async function submitPassword() {
    if (pending || !password.trim()) return;
    pending = true; inlineError = '';
    try { handToPhase(await wizardSubmitPassword(password, flowId, csrf)); }
    catch (cause) { fail(cause); }
    finally { pending = false; }
  }

  async function restart() {
    if (pending) return;
    pending = true; inlineError = '';
    try {
      await wizardCancel(flowId, csrf);
      flowId = newFlowId(); phase = 'idle'; code = ''; password = '';
      await tick(); phoneInput?.focus();
    } catch (cause) { fail(cause); }
    finally { pending = false; }
  }

  async function cancel() {
    if (pending) return;
    pending = true;
    try { await wizardCancel(flowId, csrf); } catch { /* local close still wins */ }
    finally { pending = false; restoreFocus(); onClose(); }
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<div class="modal-backdrop wizard-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && void cancel()}>
  <div bind:this={dialog} class="modal-card wizard" role="dialog" aria-modal="true" aria-labelledby="telegram-login-title">
    <div class="wizard-head">
      <div><p class="card-label">Telegram login</p><h2 id="telegram-login-title">Set up Telegram storage access</h2></div>
      <button class="icon-button" type="button" title="Close" aria-label="Close Telegram login" on:click={() => void cancel()} disabled={pending}>×</button>
    </div>
    <p class="wizard-desc">Each time this dialog opens, Telegram receives a fresh login attempt. Closing it cancels the attempt.</p>

    {#if phase === 'idle'}
      <div class="wizard-step">
        <label><span>Phone (international format)</span><input bind:this={phoneInput} bind:value={phone} type="tel" placeholder="+1 555 000 0000" autocomplete="tel" /></label>
        <button class="primary" type="button" on:click={sendCode} disabled={pending || !phone.trim()}>{pending ? 'Sending…' : 'Send code'}</button>
      </div>
    {:else if phase === 'code'}
      <div class="wizard-step">
        <p class="wizard-desc">Telegram sent a confirmation code to the account above.</p>
        <label><span>Confirmation code</span><input bind:value={code} type="password" inputmode="numeric" autocomplete="one-time-code" placeholder="6-digit code" maxlength="6" /></label>
        <button class="primary" type="button" on:click={submitCode} disabled={pending || !code.trim()}>{pending ? 'Checking…' : 'Confirm'}</button>
        <button class="ghost" type="button" on:click={restart} disabled={pending}>Send a new code</button>
      </div>
    {:else if phase === 'two_fa'}
      <div class="wizard-step">
        <p class="wizard-desc">This account has two-step verification enabled. Enter its cloud password.</p>
        <label><span>Cloud password</span><input bind:value={password} type="password" autocomplete="current-password" /></label>
        <button class="primary" type="button" on:click={submitPassword} disabled={pending || !password.trim()}>{pending ? 'Authorizing…' : 'Authorize'}</button>
      </div>
    {/if}

    {#if pending}<p class="fine-print" aria-live="polite">Working…</p>{/if}
    {#if inlineError}<p class="fine-print error-hint" role="alert">{inlineError}</p>{/if}
    <div class="wizard-actions"><button class="ghost" type="button" on:click={() => void cancel()} disabled={pending}>Cancel</button></div>
  </div>
</div>

<style>
  .wizard-backdrop { z-index: 30; }
  .wizard { width: min(560px, 100%); }
  .wizard-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 1rem; }
  .wizard h2 { margin: 0.25rem 0 0; }
  .wizard-step { display: grid; gap: 1rem; padding: 1rem; border-radius: 18px; background: rgba(255, 255, 255, 0.72); border: 1px solid var(--border); }
  .wizard-step label { display: grid; gap: 0.5rem; }
  .wizard-desc { margin: 0; color: var(--muted); line-height: 1.55; }
  .wizard-actions { display: flex; justify-content: flex-end; gap: 0.75rem; }
</style>
