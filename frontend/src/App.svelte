<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getBootstrapStatus,
    getOverview,
    getSession,
    login,
    logout,
    refreshSession,
    runOnboardingCheck
  } from './lib/api';
  import type { BootstrapState, OverviewState, SessionState } from './lib/types';

  let session: SessionState | null = null;
  let overview: OverviewState | null = null;
  let bootstrap: BootstrapState | null = null;
  let bootstrapSecret = '';
  let loading = true;
  let busy = false;
  let message = '';
  let error = '';

  const numberFormatter = new Intl.NumberFormat('en-US');

  onMount(() => {
    void bootstrapApp();
  });

  async function bootstrapApp() {
    loading = true;
    error = '';
    try {
      [session, bootstrap] = await Promise.all([getSession(), getBootstrapStatus()]);
      if (session.authenticated) {
        overview = await getOverview();
      } else {
        overview = null;
      }
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      loading = false;
    }
  }

  async function handleLogin() {
    busy = true;
    error = '';
    message = '';
    try {
      session = await login(bootstrapSecret.trim());
      bootstrapSecret = '';
      overview = await getOverview();
      bootstrap = await getBootstrapStatus();
      message = 'Operator session established.';
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  async function handleRefresh() {
    if (!session?.csrf_token) return;
    busy = true;
    error = '';
    message = '';
    try {
      session = await refreshSession(session.csrf_token);
      overview = await getOverview();
      bootstrap = await getBootstrapStatus();
      message = 'Session refreshed.';
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  async function handleLogout() {
    if (!session?.csrf_token) return;
    busy = true;
    error = '';
    message = '';
    try {
      session = await logout(session.csrf_token);
      overview = null;
      bootstrap = await getBootstrapStatus();
      message = 'Logged out successfully.';
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  async function handleRecheck() {
    if (!session?.csrf_token) return;
    busy = true;
    error = '';
    message = '';
    try {
      bootstrap = await runOnboardingCheck(session.csrf_token);
      overview = await getOverview();
      message = 'Setup checks refreshed.';
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  function normalizeError(cause: unknown) {
    return cause instanceof Error ? cause.message : 'Unexpected failure';
  }

  function formatCount(value: number) {
    return numberFormatter.format(value);
  }

  function formatBytes(value: number) {
    if (!value) return '0 B';
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
    let size = value;
    let unit = 0;
    while (size >= 1024 && unit < units.length - 1) {
      size /= 1024;
      unit += 1;
    }
    return `${size.toFixed(size >= 10 || unit === 0 ? 0 : 1)} ${units[unit]}`;
  }
</script>

<svelte:head>
  <title>Telegram S3 Admin</title>
</svelte:head>

<main class="shell">
  <section class="hero">
    <div class="hero-copy">
      <p class="eyebrow">Authenticated operator console</p>
      <h1>Telegram-backed storage, visible at a glance.</h1>
      <p class="lede">
        Keep the storage overview, bootstrap checks, and Telegram readiness behind
        one browser-gated surface without adding another runtime container.
      </p>
    </div>

    <div class="hero-panel">
      <div class="panel-row">
        <span class="panel-label">Session</span>
        <span class:badge-ok={session?.authenticated} class:badge-warn={!session?.authenticated} class="badge">
          {session?.authenticated ? 'Authenticated' : 'Locked'}
        </span>
      </div>
      <div class="panel-row">
        <span class="panel-label">Bootstrap</span>
        <span class:badge-ok={bootstrap?.ready} class:badge-warn={!bootstrap?.ready} class="badge">
          {bootstrap?.ready ? 'Ready' : 'Needs attention'}
        </span>
      </div>
      <div class="panel-row">
        <span class="panel-label">Telegram</span>
        <span class="panel-value">{bootstrap?.session_state ?? 'unknown'}</span>
      </div>
    </div>
  </section>

  {#if loading}
    <section class="card surface">
      <p>Loading admin status…</p>
    </section>
  {:else if !session?.authenticated}
    <section class="login-grid">
      <div class="card surface intro-card">
        <p class="card-label">First-run access</p>
        <h2>Unlock the operator console</h2>
        <p>
          Use the bootstrap secret configured in the environment to enroll the
          first operator session. The Telegram login itself still happens through
          the existing server-side auth flow.
        </p>

        <ol class="setup-steps">
          <li>Confirm the phone number and required `.env` values.</li>
          <li>Complete Telegram login, including 2FA if the account requires it.</li>
          <li>Return here and run the connection checks again.</li>
        </ol>

        <div class="callout">
          <strong>What this panel checks</strong>
          <ul>
            {#each bootstrap?.checks ?? [] as check}
              <li class:check-fail={!check.ok}>{check.label}: {check.detail}</li>
            {/each}
          </ul>
        </div>
      </div>

      <form class="card form-card surface" on:submit|preventDefault={handleLogin}>
        <label>
          <span>Bootstrap secret</span>
          <input
            bind:value={bootstrapSecret}
            type="password"
            autocomplete="current-password"
            placeholder="Enter the admin bootstrap secret"
          />
        </label>

        <button class="primary" type="submit" disabled={busy || !bootstrapSecret.trim()}>
          {busy ? 'Signing in…' : 'Sign in'}
        </button>
        <p class="fine-print">
          Keep this secret off browser storage. The session cookie remains
          HTTP-only and path-scoped to <code>/_admin</code>.
        </p>
      </form>
    </section>
  {:else}
    <section class="toolbar card surface">
      <div>
        <p class="card-label">Session expires</p>
        <strong>{session.expires_at ?? 'unknown'}</strong>
      </div>
      <div class="toolbar-actions">
        <button on:click={handleRecheck} disabled={busy}>Recheck onboarding</button>
        <button on:click={handleRefresh} disabled={busy}>Refresh session</button>
        <button class="ghost" on:click={handleLogout} disabled={busy}>Logout</button>
      </div>
    </section>

    <section class="cards">
      <article class="card metric">
        <p class="card-label">Buckets</p>
        <strong>{formatCount(overview?.storage.buckets ?? 0)}</strong>
      </article>
      <article class="card metric">
        <p class="card-label">Committed objects</p>
        <strong>{formatCount(overview?.storage.committed_objects ?? 0)}</strong>
      </article>
      <article class="card metric">
        <p class="card-label">Staged objects</p>
        <strong>{formatCount(overview?.storage.staged_objects ?? 0)}</strong>
      </article>
      <article class="card metric">
        <p class="card-label">Recovery markers</p>
        <strong>{formatCount(overview?.storage.recovery_markers ?? 0)}</strong>
      </article>
    </section>

    <section class="layout">
      <article class="card surface">
        <p class="card-label">Endpoint details</p>
        <h2>Runtime listeners</h2>
        <dl class="kv">
          <div>
            <dt>S3 listener</dt>
            <dd>{overview?.endpoint.s3_bind_addr}</dd>
          </div>
          <div>
            <dt>Admin listener</dt>
            <dd>{overview?.endpoint.admin_bind_addr}</dd>
          </div>
          <div>
            <dt>Admin route</dt>
            <dd>{overview?.endpoint.admin_route_prefix}</dd>
          </div>
          <div>
            <dt>Updated</dt>
            <dd>{overview?.checked_at}</dd>
          </div>
        </dl>
      </article>

      <article class="card surface">
        <p class="card-label">Capacity</p>
        <h2>Storage envelope</h2>
        <dl class="kv">
          <div>
            <dt>Chunk size</dt>
            <dd>{formatBytes(overview?.capacity.chunk_size ?? 0)}</dd>
          </div>
          <div>
            <dt>Recovery-required objects</dt>
            <dd>{formatCount(overview?.capacity.recovery_required_objects ?? 0)}</dd>
          </div>
          <div>
            <dt>Orphaned chunks</dt>
            <dd>{formatCount(overview?.capacity.orphaned_chunks ?? 0)}</dd>
          </div>
          <div>
            <dt>Data dir</dt>
            <dd>{overview?.storage.data_dir}</dd>
          </div>
        </dl>
      </article>
    </section>

    <section class="layout">
      <article class="card surface">
        <p class="card-label">Telegram</p>
        <h2>Transport health</h2>
        <dl class="kv">
          <div>
            <dt>Session state</dt>
            <dd>{overview?.telegram.session_state}</dd>
          </div>
          <div>
            <dt>Proxy mode</dt>
            <dd>{bootstrap?.proxy_mode}</dd>
          </div>
          <div>
            <dt>Proxy URL</dt>
            <dd>{bootstrap?.proxy_url ?? overview?.telegram.proxy_url ?? 'none'}</dd>
          </div>
          <div>
            <dt>Phone number</dt>
            <dd>{overview?.telegram.phone_number ?? bootstrap?.phone_number ?? 'not set'}</dd>
          </div>
        </dl>
      </article>

      <article class="card surface">
        <p class="card-label">Bootstrap checks</p>
        <h2>First-run readiness</h2>
        <ul class="checks">
          {#each bootstrap?.checks ?? [] as check}
            <li class:check-ok={check.ok} class:check-fail={!check.ok}>
              <span>{check.label}</span>
              <small>{check.detail}</small>
            </li>
          {/each}
        </ul>
      </article>
    </section>
  {/if}

  {#if message}
    <section class="toast success">{message}</section>
  {/if}

  {#if error}
    <section class="toast error">{error}</section>
  {/if}
</main>
