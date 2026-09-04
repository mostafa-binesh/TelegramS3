<script lang="ts">
  import {
    wizardBegin,
    wizardCancel,
    wizardSubmitCode,
    wizardSubmitPassword
  } from '../lib/api';
  import type { WizardPhase } from '../lib/types';

  export let csrf: string | null | undefined;
  export let onDone: () => void = () => {};

  let closed = false;
  let pending = false;
  let phase: WizardPhase = 'idle';
  let inlineError = '';

  let phone = '';
  let code = '';
  let password = '';

  function handToPhase(state: { phase: WizardPhase; message?: string | null }) {
    phase = state.phase;
    inlineError = '';
    if (state.phase === 'idle') {
      code = '';
      password = '';
    }
    if (state.phase === 'two_fa') {
      code = '';
    }
    if (state.phase === 'authorized') {
      code = '';
      password = '';
      closed = true;
      onDone();
    }
  }

  function fail(cause: unknown) {
    inlineError = cause instanceof Error ? cause.message : 'Something went wrong';
  }

  async function sendCode() {
    if (pending) return;
    pending = true;
    inlineError = '';
    try {
      const state = await wizardBegin(phone.trim() || undefined, csrf);
      handToPhase(state);
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
      const state = await wizardSubmitCode(code.trim(), csrf);
      handToPhase(state);
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
      const state = await wizardSubmitPassword(password, csrf);
      handToPhase(state);
    } catch (cause) {
      fail(cause);
    } finally {
      pending = false;
    }
  }

  async function cancel() {
    if (pending) return;
    closed = true;
    try {
      if (csrf) await wizardCancel(csrf);
    } catch {
      // best-effort teardown; the server clears its state before a fresh begin
    } finally {
      pending = false;
    }
  }
</script>

{#if !closed}
  <article class="card surface wizard">
    <div class="wizard-head">
      <div>
        <p class="card-label">Telegram login</p>
        <h3>Set up Telegram storage access</h3>
      </div>
      <button class="icon-button" type="button" title="Dismiss" on:click={cancel}>×</button>
    </div>

    <ol class="setup-steps">
      <li>Connect the operator to Telegram once; credentials ride the authenticated session.</li>
      <li>Authorize so the store can write chunks to the storage chat.</li>
      <li>Credentials only ever travel over the authenticated session.</li>
    </ol>

    {#if phase === 'idle'}
      <div class="wizard-step">
        <p class="wizard-desc">
          Add the phone number of the Telegram account whose storage chat this store writes to,
          or leave it blank to type it directly here.
        </p>
        <label>
          <span>Phone (international format, optional)</span>
          <input bind:value={phone} type="tel" placeholder="+1 555 000 0000" />
        </label>
        <button class="primary" type="button" on:click={sendCode} disabled={pending}>
          Send code
        </button>
        <div class="wizard-actions">
          <button class="ghost" type="button" on:click={cancel} disabled={pending}>Cancel</button>
        </div>
      </div>
    {:else if phase === 'code'}
      <div class="wizard-step">
        <p class="wizard-desc">Telegram sent a confirmation code to the account above.</p>
        <label>
          <span>Confirmation code</span>
          <input
            bind:value={code}
            type="password"
            inputmode="numeric"
            autocomplete="one-time-code"
            placeholder="6-digit code"
            maxlength="6"
          />
        </label>
        <button class="primary" type="button" on:click={submitCode} disabled={pending || !code.trim()}>
          Confirm
        </button>
        <div class="wizard-actions">
          <button class="ghost" type="button" on:click={cancel} disabled={pending}>Cancel</button>
        </div>
      </div>
    {:else if phase === 'two_fa'}
      <div class="wizard-step">
        <p class="wizard-desc">
          The account has two-step verification enabled. Enter its cloud/self-destruct password —
          it is sent once over this authenticated TLS session.
        </p>
        <label>
          <span>Password / cloud password</span>
          <input bind:value={password} type="password" autocomplete="current-password" />
        </label>
        <button
          class="primary"
          type="button"
          on:click={submitPassword}
          disabled={pending || !password.trim()}
        >
          Authorize
        </button>
        <div class="wizard-actions">
          <button class="ghost" type="button" on:click={cancel} disabled={pending}>Cancel</button>
        </div>
      </div>
    {/if}

    {#if pending}
      <p class="fine-print">Working…</p>
    {/if}
    {#if inlineError}
      <p class="fine-print error-hint">{inlineError}</p>
    {/if}
  </article>
{/if}

<style>
  .wizard {
    margin-bottom: 1rem;
  }
  .wizard-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
  }
  .wizard h3 {
    margin: 0.25rem 0 0;
  }
  .icon-button {
    flex: 0 0 auto;
    line-height: 1;
    padding: 0 0.7rem;
  }
  .wizard-step {
    display: grid;
    gap: 1rem;
    margin-top: 1rem;
    padding: 1rem;
    border-radius: 18px;
    background: rgba(255, 255, 255, 0.72);
    border: 1px solid var(--border);
  }
  .wizard-step label {
    display: grid;
    gap: 0.5rem;
  }
  .wizard-actions {
    display: flex;
    gap: 0.75rem;
  }
  .wizard-desc {
    margin: 0;
    color: var(--muted);
  }
  .error-hint {
    color: var(--danger, #b00020);
  }
</style>
