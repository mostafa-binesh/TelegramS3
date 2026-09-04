<script lang="ts">
  import { onMount } from 'svelte';
  import {
    createBucket,
    createFolder,
    createUser,
    deleteBucket,
    deleteUser,
    getOverview,
    getSession,
    listBuckets,
    listObjects,
    listUsers,
    login,
    logout,
    removeObject,
    contentUrl
  } from './lib/api';
  import type {
    BucketInfo,
    ObjectEntry,
    ObjectsState,
    OverviewState,
    RecoveryIssue,
    SessionState,
    UserInfo
  } from './lib/types';
  import TelegramWizard from './components/TelegramWizard.svelte';
  import UploadBox from './components/UploadBox.svelte';

  let session: SessionState | null = null;
  let overview: OverviewState | null = null;
  let view: 'overview' | 'users' | 'buckets' = 'overview';
  let loading = true;
  let busy = false;
  let message = '';
  let error = '';

  let username = '';
  let password = '';
  let loginError = '';

  let users: UserInfo[] = [];
  let newUsername = '';
  let newDisplay = '';
  let newPassword = '';
  let newRole = 'admin';

  let buckets: BucketInfo[] = [];
  let newBucket = '';
  let selectedBucket = '';
  let currentPrefix = '';
  let listing: ObjectsState | null = null;
  let newFolder = '';
  let recoveryOpen = true;

  let showWizard = false;
  $: canManageOperators = session?.user?.role === 'superadmin';

  function telegramNeedsSetup(): boolean {
    return (overview?.telegram?.connection_state ?? 'needs_reauth') !== 'connected';
  }
  function telegramStatusLabel() {
    return overview?.telegram?.connection_state?.replaceAll('_', ' ') ?? 'needs reauth';
  }

  onMount(() => {
    void bootstrapApp();
  });

  function normalizeError(cause: unknown) {
    return cause instanceof Error ? cause.message : 'Unexpected failure';
  }
  function formatCount(value: number) {
    return new Intl.NumberFormat('en-US').format(value);
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
  function crumbs() {
    return currentPrefix.split('/').filter(Boolean);
  }

  function recoveryLabel(issue: RecoveryIssue) {
    if (issue.path) return issue.path;
    if (issue.bucket && issue.key) return `${issue.bucket}/${issue.key}`;
    if (issue.bucket) return issue.bucket;
    return issue.kind;
  }

  async function bootstrapApp() {
    loading = true;
    error = '';
    try {
      session = await getSession();
      if (session?.authenticated) {
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
    loginError = '';
    message = '';
    try {
      session = await login(username.trim(), password);
      username = '';
      password = '';
      overview = await getOverview();
      message = `Signed in as ${session?.user?.username}.`;
    } catch (cause) {
      loginError = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  async function refreshOverview() {
    try {
      overview = await getOverview();
    } catch (cause) {
      error = normalizeError(cause);
    }
  }

  async function handleLogout() {
    if (!session?.csrf_token) return;
    busy = true;
    error = '';
    try {
      session = await logout(session.csrf_token);
      overview = null;
      users = [];
      buckets = [];
      listing = null;
      showWizard = false;
      view = 'overview';
      message = 'Signed out.';
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  function toggleWizard(open: boolean) {
    showWizard = open;
  }

  async function handleWizardAuthorized() {
    showWizard = false;
    message = 'Telegram account authorized.';
    await refreshOverview();
  }

  async function switchView(next: 'overview' | 'users' | 'buckets') {
    view = next;
    error = '';
    if (next === 'overview') await refreshOverview();
    if (next === 'users') await refreshUsers();
    if (next === 'buckets') {
      await refreshBuckets();
      if (selectedBucket) await refreshObjects();
    }
  }

  async function refreshUsers() {
    const csrf = session?.csrf_token;
    try {
      const res = await listUsers(csrf);
      users = res.users ?? [];
    } catch (cause) {
      error = normalizeError(cause);
    }
  }

  async function makeUser() {
    busy = true;
    error = '';
    try {
      await createUser(session?.csrf_token, {
        username: newUsername,
        password: newPassword,
        display_name: newDisplay,
        role: newRole
      });
      newUsername = '';
      newDisplay = '';
      newPassword = '';
      message = 'User added.';
      await refreshUsers();
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  async function dropUser(id: string) {
    busy = true;
    error = '';
    try {
      await deleteUser(session?.csrf_token, id);
      message = 'User removed.';
      await refreshUsers();
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  async function refreshBuckets() {
    const csrf = session?.csrf_token;
    try {
      const res = await listBuckets(csrf);
      buckets = res.buckets ?? [];
    } catch (cause) {
      error = normalizeError(cause);
    }
  }

  async function openBucket(name: string) {
    selectedBucket = name;
    currentPrefix = '';
    view = 'buckets';
    await refreshObjects();
  }

  async function exitBucket() {
    selectedBucket = '';
    currentPrefix = '';
    listing = null;
  }

  async function refreshObjects() {
    if (!selectedBucket) return;
    const csrf = session?.csrf_token;
    try {
      listing = await listObjects(csrf, selectedBucket, currentPrefix);
    } catch (cause) {
      error = normalizeError(cause);
    }
  }

  async function makeBucket() {
    const name = newBucket.trim();
    if (!name) return;
    busy = true;
    error = '';
    try {
      await createBucket(session?.csrf_token, name);
      newBucket = '';
      message = 'Bucket created.';
      await refreshBuckets();
      await refreshOverview();
      await openBucket(name);
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  async function dropBucket(name: string) {
    if (!confirm(`Delete bucket "${name}"? It must already be empty.`)) return;
    busy = true;
    error = '';
    try {
      await deleteBucket(session?.csrf_token, name);
      if (selectedBucket === name) {
        await exitBucket();
      }
      message = 'Bucket deleted.';
      await refreshBuckets();
      await refreshOverview();
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  function enterFolder(name: string) {
    currentPrefix = `${currentPrefix}${name}/`;
    void refreshObjects();
  }

  function gotoCrumb(i: number) {
    const parts = currentPrefix.split('/').filter(Boolean).slice(0, i);
    currentPrefix = parts.map((p) => p + '/').join('');
    void refreshObjects();
  }

  async function makeFolder() {
    const name = newFolder.trim();
    if (!name || !selectedBucket) return;
    busy = true;
    error = '';
    try {
      await createFolder(session?.csrf_token, selectedBucket, `${currentPrefix}${name}/`);
      newFolder = '';
      message = 'Folder created.';
      await refreshObjects();
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }

  async function removeKey(obj: ObjectEntry | string) {
    const key = typeof obj === 'string' ? `${currentPrefix}${obj}/` : obj.key;
    busy = true;
    error = '';
    try {
      await removeObject(session?.csrf_token, selectedBucket, key);
      message = 'Deleted.';
      await refreshObjects();
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      busy = false;
    }
  }
</script>

<svelte:head>
  <title>Telegram S3 — Management</title>
</svelte:head>

<main class="shell">
  <section class="hero">
    <div class="hero-copy">
      <p class="eyebrow">Authenticated management console</p>
      <h1>Telegram-backed storage.</h1>
      <p class="lede">
        Authorize the Telegram storage account, manage operator access, and browse
        buckets and objects behind a signed-in admin session.
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
        <span class="panel-label">Signed in as</span>
        <span class="panel-value">{session?.user?.username ?? '—'}</span>
      </div>
    </div>
  </section>

  {#if loading}
    <section class="card surface"><p>Loading…</p></section>
  {:else if !session?.authenticated}
    <section class="login-grid">
      <div class="card surface intro-card">
        <p class="card-label">Operator access</p>
        <h2>Sign in to manage storage</h2>
        <p>
          Accounts live in the local metadata store and are added by an existing
          superadmin or via the <code>telegram-s3 users</code> CLI. Guests only see
          this screen.
        </p>
        <div class="callout">
          <strong>Security notes</strong>
          <ul>
            <li>All management APIs require a signed-in session.</li>
            <li>Login attempts are rate-limited and locked out after repeated failures.</li>
          </ul>
        </div>
      </div>

      <form class="card form-card surface" on:submit|preventDefault={handleLogin}>
        <label>
          <span>Username</span>
          <input bind:value={username} type="text" autocomplete="username" />
        </label>
        <label>
          <span>Password</span>
          <input bind:value={password} type="password" autocomplete="current-password" />
        </label>
        {#if loginError}
          <p class="fine-print error-hint">{loginError}</p>
        {/if}
        <button class="primary" type="submit" disabled={busy}>Sign in</button>
        <p class="fine-print">
          The session cookie is HTTP-only and scoped to <code>/_admin</code>.
        </p>
      </form>
    </section>
  {:else}
    <section class="toolbar card surface">
      <div>
        <p class="card-label">Operator</p>
        <strong>{session.user?.username}</strong>
      </div>
      <div class="toolbar-actions">
        <button class:active={view === 'overview'} on:click={() => switchView('overview')}>Overview</button>
        <button class:active={view === 'buckets'} on:click={() => switchView('buckets')}>Buckets</button>
        <button class:active={view === 'users'} on:click={() => switchView('users')}>Operators</button>
        <button class="ghost" on:click={handleLogout} disabled={busy}>Logout</button>
      </div>
    </section>

    {#if view === 'overview'}
      <article class="card surface tg-callout">
        <div class="tg-banner">
          <div class="tg-copy">
            <p class="card-label">Telegram</p>
            <h2>
              {telegramNeedsSetup()
                ? 'Telegram storage is not connected'
                : 'Telegram storage is connected'}
            </h2>
            <p>
              This wizard signs in the single Telegram account that backs storage for the
              whole server. Operator accounts are separate and live in the Operators tab.
            </p>
            <p class="fine-print">
              Storage session: {overview?.telegram?.session_state ?? 'Unknown'}
              {' '}• {telegramStatusLabel()}
            </p>
            <p class="fine-print">{overview?.telegram?.detail ?? 'No Telegram status available.'}</p>
          </div>
          <div class="tg-actions">
            <button class="primary" type="button" on:click={() => toggleWizard(true)}>
              {telegramNeedsSetup() ? 'Set up Telegram login' : 'Refresh Telegram login'}
            </button>
            <button class="ghost" type="button" on:click={() => switchView('users')}>
              Manage operators
            </button>
          </div>
        </div>
      </article>
      {#if showWizard}
        <TelegramWizard
          csrf={session?.csrf_token}
          onDone={handleWizardAuthorized}
        />
        <button class="ghost" type="button" on:click={() => toggleWizard(false)}>Close wizard</button>
      {/if}
      <section class="cards">
        <article class="card metric">
          <p class="card-label">Buckets</p>
          <strong>{formatCount(overview?.storage?.buckets ?? 0)}</strong>
        </article>
        <article class="card metric">
          <p class="card-label">Committed</p>
          <strong>{formatCount(overview?.storage?.committed_objects ?? 0)}</strong>
        </article>
        <article class="card metric">
          <p class="card-label">Active</p>
          <strong>{formatCount(overview?.storage?.active_objects ?? 0)}</strong>
        </article>
        <article class="card metric">
          <p class="card-label">Recovery</p>
          <strong>{formatCount(overview?.storage?.recovery_markers ?? 0)}</strong>
        </article>
      </section>
      <section class="card surface recovery-panel">
        <div class="section-head">
          <div>
            <p class="card-label">Recovery issues</p>
            <h2>
              {formatCount(overview?.recovery?.issue_count ?? 0)}
              {(overview?.recovery?.issue_count ?? 0) === 1 ? ' file needs attention' : ' files need attention'}
            </h2>
            <p class="fine-print">
              Click an issue to see the exact files or Telegram objects that are missing,
              unreadable, or corrupted.
            </p>
          </div>
          {#if (overview?.recovery?.issues ?? []).length > 0 || overview?.recovery?.scan_error}
            <button class="ghost" type="button" on:click={() => (recoveryOpen = !recoveryOpen)}>
              {recoveryOpen ? 'Hide details' : 'Show details'}
            </button>
          {/if}
        </div>
        {#if overview?.recovery?.scan_error}
          <p class="error-hint">Recovery scan unavailable: {overview.recovery.scan_error}</p>
        {:else if (overview?.recovery?.issue_count ?? 0) === 0}
          <p class="fine-print">No missing or corrupted files were detected.</p>
        {:else if recoveryOpen}
          <div class="recovery-list">
            {#each overview?.recovery?.issues ?? [] as issue}
              <details class="recovery-item" open>
                <summary>
                  <span>{recoveryLabel(issue)}</span>
                  <small>{issue.summary}</small>
                </summary>
                <div class="recovery-meta">
                  <span>{issue.kind}</span>
                  {#if issue.commit_state}
                    <span>{issue.commit_state}</span>
                  {/if}
                  {#if issue.object_id}
                    <span>{issue.object_id}</span>
                  {/if}
                </div>
                <ul>
                  {#each issue.details as detail}
                    <li>{detail}</li>
                  {/each}
                </ul>
              </details>
            {/each}
          </div>
        {/if}
      </section>
      <section class="layout">
        <article class="card surface">
          <p class="card-label">Endpoints</p>
          <dl class="kv">
            <div><dt>S3 listener</dt><dd>{overview?.endpoint?.s3_bind_addr}</dd></div>
            <div><dt>Admin listener</dt><dd>{overview?.endpoint?.admin_bind_addr}</dd></div>
            <div><dt>Admin route</dt><dd>{overview?.endpoint?.admin_route_prefix}</dd></div>
          </dl>
        </article>
        <article class="card surface">
          <p class="card-label">Readiness</p>
          <ul class="checks">
            {#each overview?.checks ?? [] as check}
              <li class:check-ok={check.ok} class:check-fail={!check.ok}>
                <span>{check.label}</span>
                <small>{check.detail}</small>
              </li>
            {/each}
          </ul>
        </article>
      </section>
    {:else if view === 'buckets'}
      <section class="card surface">
        <div class="section-head">
          <div>
            <p class="card-label">Buckets and files</p>
            <h2>{selectedBucket ? selectedBucket : 'Choose or create a bucket'}</h2>
          </div>
          <div class="toolbar-actions">
            <button class="ghost" type="button" on:click={refreshBuckets} disabled={busy}>Refresh</button>
            {#if selectedBucket}
              <button class="ghost" type="button" on:click={() => dropBucket(selectedBucket)} disabled={busy}>
                Delete empty bucket
              </button>
            {/if}
          </div>
        </div>
        {#if !selectedBucket}
          <form class="row-inline" on:submit|preventDefault={makeBucket}>
            <input bind:value={newBucket} placeholder="new bucket name" />
            <button class="primary" type="submit" disabled={busy || !newBucket.trim()}>
              Create bucket
            </button>
          </form>
          <p class="fine-print">
            The browser only shows existing buckets. Create one here or through any S3
            client, then open it to browse files.
          </p>
          <ul class="checks">
            {#each buckets as bucket}
              <li>
                <div class="bucket-row">
                  <button type="button" class="btn-link" on:click={() => openBucket(bucket.name)}>
                    {bucket.name}
                    <small>- created {bucket.created_at}</small>
                  </button>
                  <button class="ghost" type="button" on:click={() => dropBucket(bucket.name)} disabled={busy}>
                    Delete
                  </button>
                </div>
              </li>
            {/each}
            {#if buckets.length === 0}
              <li class="fine-print">No buckets yet. Create one above to start the file browser.</li>
            {/if}
          </ul>
        {:else}
          <div class="crumb-row">
            <button class="btn-link" on:click={exitBucket}>Bucket: {selectedBucket}</button>
            <span class="crumb-sep">/</span>
            {#each crumbs() as crumb, i (crumb + i)}
              <button class="btn-link" on:click={() => gotoCrumb(i)}>{crumb}</button><span class="crumb-sep">/</span>
            {/each}
          </div>
          <div class="row-inline">
            <input bind:value={newFolder} placeholder="new folder" />
            <button class="primary" disabled={busy || !newFolder.trim()} on:click={makeFolder}>New folder</button>
          </div>
          <UploadBox
            bucket={selectedBucket}
            prefix={currentPrefix}
            csrf={session?.csrf_token}
            onUploaded={() => refreshObjects()}
          />
          <table class="kv-table">
            <thead><tr><th>Name</th><th>Size</th><th>Modified</th><th></th></tr></thead>
            <tbody>
              {#each listing?.folders ?? [] as folder}
                <tr>
                  <td><button class="btn-link" on:click={() => enterFolder(folder)}>{folder}/</button></td>
                  <td class="muted">folder</td>
                  <td class="muted">—</td>
                  <td class="row-actions">
                    <button class="ghost" on:click={() => removeKey(folder)}>Delete</button>
                  </td>
                </tr>
              {/each}
              {#each listing?.objects ?? [] as obj}
                <tr>
                  <td>{obj.name}</td>
                  <td>{formatBytes(obj.size)}</td>
                  <td>{obj.last_modified}</td>
                  <td class="row-actions">
                    <a class="row-download" href={contentUrl(selectedBucket, obj.key)} download>
                      Download
                    </a>
                    <button class="ghost" on:click={() => removeKey(obj)}>Delete</button>
                  </td>
                </tr>
              {/each}
              {#if listing && listing.folders.length === 0 && listing.objects.length === 0}
                <tr><td colspan="4" class="muted">Empty folder.</td></tr>
              {/if}
            </tbody>
          </table>
          <p class="fine-print">
            Individual files upload and download in place here. Bulk download of a whole
            folder or bucket is a future item; for now list, navigate and manage folders
            and objects via the S3 data plane.
          </p>
        {/if}
      </section>
    {:else if view === 'users'}
      <section class="card surface">
        <p class="card-label">Operators</p>
        <h2>Accounts</h2>
        <p class="fine-print">
          These are dashboard operator accounts, not Telegram contacts. The Telegram
          storage login lives on the Overview page.
        </p>
        <table class="kv-table">
          <thead><tr><th>Username</th><th>Role</th><th>State</th><th></th></tr></thead>
          <tbody>
            {#each users as user}
              <tr>
                <td>{user.username}{#if user.display_name} <small>({user.display_name})</small>{/if}</td>
                <td>{user.role}</td>
                <td>{user.disabled ? 'disabled' : 'enabled'}</td>
                <td>
                  {#if canManageOperators}
                    <button class="ghost" on:click={() => dropUser(user.id)} disabled={busy}>Remove</button>
                  {/if}
                </td>
              </tr>
            {/each}
            {#if users.length === 0}
              <tr><td colspan="4" class="muted">No operator accounts.</td></tr>
            {/if}
          </tbody>
        </table>

        <div class="nested-form">
          <p class="card-label">Add account</p>
          {#if canManageOperators}
            <div class="grid-2">
              <label>
                <span>Username</span>
                <input bind:value={newUsername} autocomplete="off" />
              </label>
              <label>
                <span>Display name</span>
                <input bind:value={newDisplay} autocomplete="off" />
              </label>
              <label>
                <span>Password (12+ chars)</span>
                <input bind:value={newPassword} type="password" autocomplete="new-password" />
              </label>
              <label>
                <span>Role</span>
                <select bind:value={newRole}>
                  <option value="admin">admin</option>
                  <option value="superadmin">superadmin</option>
                </select>
              </label>
            </div>
            <button class="primary" on:click={makeUser} disabled={busy || !newUsername || !newPassword}>
              Add operator
            </button>
          {:else}
            <p class="fine-print">Only superadmins can add or remove operator accounts.</p>
          {/if}
        </div>
      </section>
    {/if}
  {/if}

  {#if message}
    <section class="toast success">{message}</section>
  {/if}
  {#if error}
    <section class="toast error">{error}</section>
  {/if}
</main>

<style>
  .active {
    font-weight: 700;
    text-decoration: underline;
  }
  .error-hint {
    color: var(--danger, #b00020);
  }
  .row-inline {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
    margin: 12px 0;
  }
  .row-inline input {
    flex: 1;
  }
  .grid-2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    margin: 10px 0;
  }
  .nested-form {
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid color-mix(in srgb, var(--text, #172033) 10%, transparent);
  }
  .kv-table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 8px;
  }
  .kv-table th,
  .kv-table td {
    text-align: left;
    padding: 6px 8px;
    border-bottom: 1px solid color-mix(in srgb, var(--text, #172033) 14%, transparent);
  }
  .btn-link {
    background: none;
    border: none;
    color: var(--accent, #0d7a6d);
    cursor: pointer;
    padding: 0;
    text-align: left;
    font: inherit;
  }
  .crumb-row {
    display: flex;
    flex-wrap: wrap;
    gap: 2px;
    align-items: center;
    margin-bottom: 6px;
  }
  .crumb-sep {
    margin: 0 2px;
    color: var(--text, #172033);
    opacity: 0.4;
  }
  .muted {
    color: var(--text, #172033);
    opacity: 0.5;
  }
  .row-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    white-space: nowrap;
  }
  .row-actions button {
    padding: 0.3rem 0.7rem;
  }
  .row-download {
    color: var(--accent, #0d7a6d);
    text-decoration: none;
    font-weight: 700;
  }
  .row-download:hover {
    text-decoration: underline;
  }
  .section-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
  .section-head h2 {
    margin: 0.25rem 0 0;
  }
  .bucket-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
  }
  .tg-callout {
    margin-bottom: 1rem;
  }
  .tg-banner {
    display: flex;
    flex-wrap: wrap;
    gap: 1rem;
    align-items: center;
    justify-content: space-between;
  }
  .tg-callout h2 {
    margin: 0.25rem 0 0.5rem;
    font-size: 1.3rem;
  }
  .tg-copy {
    flex: 1 1 24rem;
  }
  .tg-copy p {
    margin: 0;
    color: var(--text, #172033);
    opacity: 0.72;
    max-width: 62ch;
  }
  .tg-actions {
    display: flex;
    gap: 0.75rem;
    flex-wrap: wrap;
    align-items: center;
  }
  .recovery-panel {
    margin: 1rem 0 1.25rem;
  }
  .recovery-list {
    display: grid;
    gap: 0.75rem;
    margin-top: 1rem;
  }
  .recovery-item {
    border: 1px solid color-mix(in srgb, var(--text, #172033) 12%, transparent);
    border-radius: 16px;
    padding: 0.85rem 1rem;
    background: color-mix(in srgb, var(--bg, #ffffff) 92%, transparent);
  }
  .recovery-item summary {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 1rem;
    align-items: baseline;
    cursor: pointer;
    list-style: none;
  }
  .recovery-item summary::-webkit-details-marker {
    display: none;
  }
  .recovery-item summary span {
    font-weight: 700;
  }
  .recovery-item summary small {
    color: var(--text, #172033);
    opacity: 0.72;
  }
  .recovery-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin: 0.55rem 0 0.35rem;
    color: var(--text, #172033);
    opacity: 0.6;
    font-size: 0.9rem;
  }
  .recovery-item ul {
    margin: 0.4rem 0 0;
    padding-left: 1.2rem;
    color: var(--text, #172033);
    opacity: 0.8;
  }
</style>
