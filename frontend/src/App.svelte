<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from './components/Sidebar.svelte';
  import HealthBadge from './components/HealthBadge.svelte';
  import SetupWizard from './components/SetupWizard.svelte';
  import Transfers from './components/Transfers.svelte';
  import {getSetup} from './lib/api';
  import {
    createBucket,
    createFolder,
    createUser,
    deleteBucket,
    deleteUser,
    getOverview,
    getTelegramSettings,
    getSession,
    listBuckets,
    listObjects,
    listUsers,
    login,
    logout,
    removeObject,
    contentUrl,
    saveTelegramSettings
  } from './lib/api';
  import {formatBytes, formatCount, formatTimestamp, normalizeError} from './lib/format';
  import type {
    BucketInfo,
    ObjectEntry,
    ObjectsState,
    OverviewState,
    SessionState,
    TelegramSettings,
    UserInfo
  } from './lib/types';
  import TelegramWizard from './components/TelegramWizard.svelte';
  import UploadBox from './components/UploadBox.svelte';
  import RecoveryIssues from './components/RecoveryIssues.svelte';
  import TopProgress from './components/TopProgress.svelte';

  let session: SessionState | null = null;
  let overview: OverviewState | null = null;
  let view: 'overview' | 'users' | 'buckets' | 'transfers' | 'recovery' | 'telegram' = 'overview';
  let loading = true;
  let setupRequired = false;
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

  // Per-area busy flags. Only operator-initiated loads set these; the background
  // poll refreshes silently so the console never flickers on its own.
  let overviewLoading = false;
  let usersLoading = false;
  let bucketsLoading = false;
  let objectsLoading = false;
  $: anyLoading = overviewLoading || usersLoading || bucketsLoading || objectsLoading;

  let showWizard = false;
  let telegramApiId = '';
  let telegramApiHash = '';
  let telegramStorageChatId = '';
  let telegramProxyUrl = '';
  let telegramProxyUsername = '';
  let telegramProxyPassword = '';
  let telegramProxyMode = 'auto';
  let telegramSettingsBusy = false;
  let telegramSettingsError = '';
  let telegramSettingsMessage = '';
  $: canManageOperators = session?.user?.role === 'superadmin';
  // The Overview leads with the actionable count; acknowledged issues stay
  // visible on the Recovery page but stop counting here.
  $: corruptedCount = overview?.recovery?.unacknowledged_count ?? 0;
  $: acknowledgedCount =
    (overview?.recovery?.issue_count ?? 0) - (overview?.recovery?.unacknowledged_count ?? 0);

  function telegramNeedsSetup(): boolean {
    return (overview?.telegram?.connection_state ?? 'needs_reauth') !== 'connected';
  }
  function telegramStatusLabel() {
    return overview?.telegram?.connection_state?.replaceAll('_', ' ') ?? 'needs reauth';
  }

  onMount(() => {
    void bootstrapApp();
    let disposed=false; let timer:ReturnType<typeof setTimeout>;
    const poll=async()=>{if(session?.authenticated&&!document.hidden)await refreshOverview({silent:true});if(!disposed)timer=setTimeout(poll,error?30000:10000);};
    timer=setTimeout(poll,10000);
    return ()=>{disposed=true;clearTimeout(timer);};
  });

  function crumbs() {
    return currentPrefix.split('/').filter(Boolean);
  }

  async function bootstrapApp() {
    loading = true;
    error = '';
    try {
      session = await getSession();
      if(!session.authenticated) setupRequired=(await getSetup()).setup_required;
      if (session?.authenticated) {
        overview = await getOverview();
        await refreshTelegramSettings();
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

  /** `silent` keeps the background poll from lighting up the loading UI. */
  async function refreshOverview(options: {silent?: boolean} = {}) {
    if (!options.silent) overviewLoading = true;
    try {
      overview = await getOverview();
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      overviewLoading = false;
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
    await refreshTelegramSettings();
  }

  async function refreshTelegramSettings() {
    try {
      const response = await getTelegramSettings();
      applyTelegramSettings(response.settings);
    } catch (cause) {
      telegramSettingsError = normalizeError(cause);
    }
  }

  function applyTelegramSettings(settings: TelegramSettings) {
    telegramApiId = settings.telegram_api_id ?? '';
    telegramApiHash = settings.telegram_api_hash ?? '';
    telegramStorageChatId = settings.telegram_storage_chat_id ?? '';
    telegramProxyUrl = settings.telegram_proxy_url ?? '';
    telegramProxyUsername = settings.telegram_proxy_username ?? '';
    telegramProxyPassword = settings.telegram_proxy_password ?? '';
    telegramProxyMode = settings.telegram_proxy_mode ?? 'auto';
    telegramSettingsError = '';
  }

  async function saveTelegramSettingsForm() {
    telegramSettingsBusy = true;
    telegramSettingsError = '';
    telegramSettingsMessage = '';
    try {
      const response = await saveTelegramSettings(session?.csrf_token, {
        telegram_api_id: telegramApiId.trim(),
        telegram_api_hash: telegramApiHash.trim(),
        telegram_storage_chat_id: telegramStorageChatId.trim(),
        telegram_proxy_url: telegramProxyUrl.trim(),
        telegram_proxy_username: telegramProxyUsername.trim(),
        telegram_proxy_password: telegramProxyPassword,
        telegram_proxy_mode: telegramProxyMode.trim()
      });
      applyTelegramSettings(response.settings);
      telegramSettingsMessage = response.refresh_error
        ? `Telegram settings saved. Refresh warning: ${response.refresh_error}`
        : 'Telegram settings saved.';
      await refreshOverview();
    } catch (cause) {
      telegramSettingsError = normalizeError(cause);
    } finally {
      telegramSettingsBusy = false;
    }
  }

  async function switchView(next: 'overview' | 'users' | 'buckets' | 'transfers' | 'recovery' | 'telegram') {
    view = next;
    error = '';
    if (next === 'overview' || next === 'recovery') await refreshOverview();
    if (next === 'telegram') await refreshTelegramSettings();
    if (next === 'users') await refreshUsers();
    if (next === 'buckets') {
      await refreshBuckets();
      if (selectedBucket) await refreshObjects();
    }
  }

  async function refreshUsers() {
    const csrf = session?.csrf_token;
    usersLoading = true;
    try {
      const res = await listUsers(csrf);
      users = res.users ?? [];
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      usersLoading = false;
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
    bucketsLoading = true;
    try {
      const res = await listBuckets(csrf);
      buckets = res.buckets ?? [];
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      bucketsLoading = false;
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
    objectsLoading = true;
    try {
      listing = await listObjects(csrf, selectedBucket, currentPrefix);
    } catch (cause) {
      error = normalizeError(cause);
    } finally {
      objectsLoading = false;
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
      // Stay on the list: the new bucket appears there, and the operator decides
      // when to open it.
      message = `Bucket "${name}" created.`;
      await refreshBuckets();
      await refreshOverview();
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

<TopProgress active={anyLoading}/>

<main class="shell" class:signed-in={session?.authenticated}>
  {#if session?.authenticated}
    <Sidebar {view} username={session.user?.username ?? ''} onNavigate={switchView} onLogout={handleLogout}/>
    <header class="console-header"><div><p class="card-label">Workspace</p><h1>{view==='users'?'Operators':view==='telegram'?'Telegram settings':view.charAt(0).toUpperCase()+view.slice(1)}</h1></div><HealthBadge state={overview?.telegram?.connection_state ?? 'checking'} detail={overview?.telegram?.detail ?? 'Waiting for a connection check'} checkedAt={overview?.checked_at}/></header>
  {/if}
  {#if loading}
    <section class="card surface"><p>Loading…</p></section>
  {:else if setupRequired && !session?.authenticated}
    <SetupWizard onCreated={(created)=>{session=created;setupRequired=false;view='telegram';void refreshOverview();void refreshTelegramSettings();}}/>
  {:else if !session?.authenticated}
    <section class="login-grid">
      <div class="card surface intro-card">
        <p class="card-label">Operator access</p>
        <h2>Sign in to manage storage</h2>
        <p>Sign in to browse files, follow transfers, and manage your Telegram storage connection.</p>
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

      </form>
    </section>
  {:else}
    {#if view === 'transfers'}
      <Transfers csrf={session?.csrf_token}/>
    {:else if view === 'recovery'}
      <RecoveryIssues
        recovery={overview?.recovery}
        csrf={session?.csrf_token}
        loading={overviewLoading}
        onChanged={() => refreshOverview({silent: true})}
        onRefresh={() => refreshOverview()}
      />
      <Transfers csrf={session?.csrf_token} recoveryOnly/>
    {:else if view === 'telegram'}
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
        <form class="telegram-settings" on:submit|preventDefault={saveTelegramSettingsForm}>
          <div class="settings-grid">
            <label>
              <span>Telegram API ID</span>
              <input bind:value={telegramApiId} type="text" autocomplete="off" />
            </label>
            <label>
              <span>Telegram API hash</span>
              <input bind:value={telegramApiHash} type="password" autocomplete="off" />
            </label>
            <label>
              <span>Storage chat ID</span>
              <input bind:value={telegramStorageChatId} type="text" autocomplete="off" />
            </label>
            <label>
              <span>Proxy mode</span>
              <input bind:value={telegramProxyMode} type="text" autocomplete="off" placeholder="auto" />
            </label>
            <label>
              <span>Proxy URL</span>
              <input bind:value={telegramProxyUrl} type="text" autocomplete="off" placeholder="socks5://127.0.0.1:12334" />
            </label>
            <label>
              <span>Proxy username</span>
              <input bind:value={telegramProxyUsername} type="text" autocomplete="off" />
            </label>
            <label>
              <span>Proxy password</span>
              <input bind:value={telegramProxyPassword} type="password" autocomplete="off" />
            </label>
          </div>
          <div class="settings-actions">
            <button class="primary" type="submit" disabled={telegramSettingsBusy}>Save Telegram settings</button>
          </div>
          {#if telegramSettingsMessage}
            <p class="fine-print">{telegramSettingsMessage}</p>
          {/if}
          {#if telegramSettingsError}
            <p class="fine-print error-hint">{telegramSettingsError}</p>
          {/if}
        </form>
      </article>
      {#if showWizard}
        <TelegramWizard
          csrf={session?.csrf_token}
          onDone={handleWizardAuthorized}
        />
        <button class="ghost" type="button" on:click={() => toggleWizard(false)}>Close wizard</button>
      {/if}
    {:else if view === 'overview'}
      <section class="section-head overview-head">
        <div>
          <p class="card-label">Snapshot</p>
          <h2>Storage at a glance</h2>
        </div>
        <button class="ghost" type="button" on:click={() => refreshOverview()} disabled={overviewLoading}>
          {#if overviewLoading}<span class="spinner" aria-hidden="true"></span>{/if}
          Refresh
        </button>
      </section>
      <section class="cards">
        {#if !overview}
          {#each [0, 1, 2, 3] as slot (slot)}
            <article class="card metric"><div class="skeleton" style="height:62px"></div></article>
          {/each}
        {:else}
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
          <article class="card metric corrupted" class:attention={corruptedCount > 0}>
            <p class="card-label">Corrupted files</p>
            <strong>{formatCount(corruptedCount)}</strong>
            {#if overview?.recovery?.scan_error}
              <small class="error-hint">Scan unavailable</small>
            {:else if acknowledgedCount > 0}
              <small>{formatCount(acknowledgedCount)} acknowledged</small>
            {:else if corruptedCount === 0}
              <small>Nothing needs attention</small>
            {/if}
            {#if corruptedCount > 0 || acknowledgedCount > 0 || overview?.recovery?.scan_error}
              <button class="btn-link card-link" type="button" on:click={() => switchView('recovery')}>
                View details →
              </button>
            {/if}
          </article>
        {/if}
      </section>
      <section class="layout">
        <article class="card surface">
          <p class="card-label">Readiness</p>
          {#if !overview}
            <div class="skeleton-stack">
              <div class="skeleton" style="height:56px"></div>
              <div class="skeleton" style="height:56px"></div>
              <div class="skeleton" style="height:56px"></div>
            </div>
          {:else}
            <ul class="checks">
              {#each overview?.checks ?? [] as check}
                <li class:check-ok={check.ok} class:check-fail={!check.ok}>
                  <span>{check.label}</span>
                  <small>{check.detail}</small>
                </li>
              {/each}
            </ul>
          {/if}
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
            <button
              class="ghost"
              type="button"
              on:click={() => (selectedBucket ? refreshObjects() : refreshBuckets())}
              disabled={busy || bucketsLoading || objectsLoading}
            >
              {#if bucketsLoading || objectsLoading}<span class="spinner" aria-hidden="true"></span>{/if}
              Refresh
            </button>
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
          {#if bucketsLoading && buckets.length === 0}
            <div class="skeleton-stack">
              <div class="skeleton" style="height:52px"></div>
              <div class="skeleton" style="height:52px"></div>
            </div>
          {:else if buckets.length === 0}
            <p class="empty-state">
              <span class="empty-mark" aria-hidden="true">+</span>
              No buckets yet. Create one above to start the file browser.
            </p>
          {:else}
            <ul class="checks">
              {#each buckets as bucket (bucket.name)}
                <li>
                  <div class="bucket-row">
                    <button type="button" class="btn-link" on:click={() => openBucket(bucket.name)}>
                      {bucket.name}
                      <small>created {formatTimestamp(bucket.created_at)}</small>
                    </button>
                    <button class="ghost" type="button" on:click={() => dropBucket(bucket.name)} disabled={busy}>
                      Delete
                    </button>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
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
          {#if objectsLoading && !listing}
            <div class="skeleton-stack">
              <div class="skeleton" style="height:40px"></div>
              <div class="skeleton" style="height:40px"></div>
              <div class="skeleton" style="height:40px"></div>
            </div>
          {:else if listing && listing.folders.length === 0 && listing.objects.length === 0}
            <p class="empty-state">
              <span class="empty-mark" aria-hidden="true">↑</span>
              This folder is empty. Drop files above to upload the first one.
            </p>
          {:else}
            <div class="table-scroll">
              <table class="kv-table">
                <thead><tr><th>Name</th><th>Size</th><th>Modified</th><th></th></tr></thead>
                <tbody>
                  {#each listing?.folders ?? [] as folder (folder)}
                    <tr>
                      <td><button class="btn-link" on:click={() => enterFolder(folder)}>{folder}/</button></td>
                      <td class="muted">folder</td>
                      <td class="muted">—</td>
                      <td class="row-actions">
                        <button class="ghost" on:click={() => removeKey(folder)}>Delete</button>
                      </td>
                    </tr>
                  {/each}
                  {#each listing?.objects ?? [] as obj (obj.key)}
                    <tr>
                      <td>{obj.name}</td>
                      <td>{formatBytes(obj.size)}</td>
                      <td>{formatTimestamp(obj.last_modified)}</td>
                      <td class="row-actions">
                        <a class="row-download" href={contentUrl(selectedBucket, obj.key)} download>
                          Download
                        </a>
                        <button class="ghost" on:click={() => removeKey(obj)}>Delete</button>
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
          <p class="fine-print">
            Individual files upload and download in place here. Bulk download of a whole
            folder or bucket is a future item; for now list, navigate and manage folders
            and objects via the S3 data plane.
          </p>
        {/if}
      </section>
    {:else if view === 'users'}
      <section class="card surface">
        <div class="section-head">
          <div>
            <p class="card-label">Operators</p>
            <h2>Accounts</h2>
            <p class="fine-print">
              These are dashboard operator accounts, not Telegram contacts. The Telegram
              storage login lives on the Telegram settings page.
            </p>
          </div>
          <button class="ghost" type="button" on:click={refreshUsers} disabled={usersLoading}>
            {#if usersLoading}<span class="spinner" aria-hidden="true"></span>{/if}
            Refresh
          </button>
        </div>
        {#if usersLoading && users.length === 0}
          <div class="skeleton-stack">
            <div class="skeleton" style="height:44px"></div>
            <div class="skeleton" style="height:44px"></div>
          </div>
        {:else if users.length === 0}
          <p class="empty-state">
            <span class="empty-mark" aria-hidden="true">+</span>
            No operator accounts yet.
          </p>
        {:else}
          <div class="table-scroll">
            <table class="kv-table">
              <thead><tr><th>Username</th><th>Role</th><th>State</th><th></th></tr></thead>
              <tbody>
                {#each users as user (user.id)}
                  <tr>
                    <td>{user.username}{#if user.display_name} <small>({user.display_name})</small>{/if}</td>
                    <td><span class="role-tag" class:role-super={user.role === 'superadmin'}>{user.role}</span></td>
                    <td>{user.disabled ? 'disabled' : 'enabled'}</td>
                    <td class="row-actions">
                      {#if canManageOperators}
                        <button class="ghost" on:click={() => dropUser(user.id)} disabled={busy}>Remove</button>
                      {/if}
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}

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
  .shell.signed-in{max-width:none;margin-left:230px;padding:30px 40px;min-height:100vh}
  .console-header{display:flex;justify-content:space-between;gap:24px;align-items:center;margin-bottom:30px}
  .console-header h1{font-size:26px;letter-spacing:-.04em;margin:4px 0}
  @media(max-width:760px){.shell.signed-in{margin-left:0;padding:0 16px 24px}.console-header{margin-top:24px;align-items:flex-start;flex-direction:column}}

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
    gap: 14px;
    margin: 14px 0;
  }
  /* Stack every field the same way, so the role select lines up with the inputs. */
  .grid-2 label {
    display: grid;
    gap: 0.45rem;
    align-content: start;
  }
  .grid-2 label span {
    font-size: 0.8rem;
    color: var(--muted);
  }
  .nested-form {
    margin-top: 16px;
    padding-top: 16px;
    border-top: 1px solid var(--border);
  }
  .table-scroll {
    overflow-x: auto;
  }
  .kv-table {
    width: 100%;
    border-collapse: collapse;
    margin-top: 8px;
  }
  .kv-table th {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
  }
  .kv-table th,
  .kv-table td {
    text-align: left;
    padding: 12px 10px;
    border-bottom: 1px solid var(--border);
  }
  .kv-table tbody tr {
    transition: background 120ms ease;
  }
  .kv-table tbody tr:hover {
    background: color-mix(in srgb, var(--accent) 5%, transparent);
  }
  .role-tag {
    display: inline-block;
    padding: 0.15rem 0.55rem;
    border-radius: 999px;
    font-size: 0.8rem;
    background: color-mix(in srgb, var(--text) 8%, transparent);
    color: var(--muted);
  }
  .role-super {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .btn-link {
    background: none;
    border: none;
    color: var(--accent);
    cursor: pointer;
    padding: 0;
    text-align: left;
    font: inherit;
  }
  .btn-link:hover {
    text-decoration: underline;
  }
  .overview-head {
    margin-bottom: 0.25rem;
  }
  .overview-head h2 {
    margin: 0.25rem 0 0;
  }
  .corrupted {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
  }
  .corrupted small {
    display: block;
    margin-top: 0.35rem;
    color: var(--muted);
  }
  .corrupted.attention {
    border-color: color-mix(in srgb, var(--danger) 40%, var(--border));
    background: color-mix(in srgb, var(--danger) 5%, var(--surface));
  }
  .corrupted.attention strong {
    color: var(--danger);
  }
  .card-link {
    margin-top: 0.6rem;
    font-size: 0.85rem;
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
    color: var(--muted);
    opacity: 0.6;
  }
  .muted {
    color: var(--muted);
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
    color: var(--accent);
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
    color: var(--muted);
    max-width: 62ch;
  }
  .tg-actions {
    display: flex;
    gap: 0.75rem;
    flex-wrap: wrap;
    align-items: center;
  }
</style>
