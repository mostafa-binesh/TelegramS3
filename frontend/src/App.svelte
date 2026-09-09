<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from './components/Sidebar.svelte';
  import HealthBadge from './components/HealthBadge.svelte';
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
    saveTelegramSettings,
    uploadObject
  } from './lib/api';
  import {normalizeError} from './lib/format';
  import type {
    BucketInfo,
    ObjectEntry,
    ObjectsState,
    OverviewState,
    SessionState,
    TelegramSettings,
    UserInfo
  } from './lib/types';
  import TopProgress from './components/TopProgress.svelte';

  let session: SessionState | null = null;
  let overview: OverviewState | null = null;
  let view: 'overview' | 'users' | 'buckets' | 'transfers' | 'recovery' | 'telegram' = 'overview';
  let loading = true;
  let setupRequired = false;
  let busy = false;
  let message = '';
  let error = '';
  let SetupWizardComponent: any = null;
  let TransfersComponent: any = null;
  let RecoveryIssuesComponent: any = null;
  let TelegramWizardComponent: any = null;
  let UploadBoxComponent: any = null;
  let UsersPanelComponent: any = null;
  let TelegramPanelComponent: any = null;
  let OverviewPanelComponent: any = null;
  let BucketsPanelComponent: any = null;
  let AdminModalsComponent: any = null;
  let messageTimer: ReturnType<typeof setTimeout> | undefined;
  $: if (message) {
    if (messageTimer) clearTimeout(messageTimer);
    messageTimer = setTimeout(() => { message = ''; }, 6000);
  }

  async function loadSetupWizard() { SetupWizardComponent ??= (await import('./components/SetupWizard.svelte')).default; }
  async function loadTransfers() { TransfersComponent ??= (await import('./components/Transfers.svelte')).default; }
  async function loadRecoveryIssues() { RecoveryIssuesComponent ??= (await import('./components/RecoveryIssues.svelte')).default; }
  async function loadTelegramWizard() { TelegramWizardComponent ??= (await import('./components/TelegramWizard.svelte')).default; }
  async function loadUploadBox() { UploadBoxComponent ??= (await import('./components/UploadBox.svelte')).default; }
  async function loadUsersPanel() { UsersPanelComponent ??= (await import('./components/UsersPanel.svelte')).default; }
  async function loadTelegramPanel() { TelegramPanelComponent ??= (await import('./components/TelegramPanel.svelte')).default; }
  async function loadOverviewPanel() { OverviewPanelComponent ??= (await import('./components/OverviewPanel.svelte')).default; }
  async function loadBucketsPanel() { BucketsPanelComponent ??= (await import('./components/BucketsPanel.svelte')).default; }
  async function loadAdminModals() { AdminModalsComponent ??= (await import('./components/AdminModals.svelte')).default; }

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
  let showBucketModal = false;
  let showFolderModal = false;
  let showUploadModal = false;
  let showOperatorModal = false;
  let showTelegramCredentials = false;
  let selectedKeys: string[] = [];
  let showMoveModal = false;
  let moveBucket = '';
  let movePrefix = '';

  // Per-area busy flags. Only operator-initiated loads set these; the background
  // poll refreshes silently so the console never flickers on its own.
  let overviewLoading = false;
  let usersLoading = false;
  let bucketsLoading = false;
  let objectsLoading = false;
  let recoveryLoading = false;
  let recoveryTab: 'issues' | 'transfers' = 'issues';
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
      if(!session.authenticated) { setupRequired=(await getSetup()).setup_required; if (setupRequired) await loadSetupWizard(); }
      if (session?.authenticated) {
        const [loadedOverview] = await Promise.all([getOverview(), loadOverviewPanel()]);
        overview = loadedOverview;
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
      const [loadedOverview] = await Promise.all([getOverview(), loadOverviewPanel()]);
      overview = loadedOverview;
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

  async function toggleWizard(open: boolean) {
    if (open) await loadTelegramWizard();
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
    if (next === 'overview') await loadOverviewPanel();
    if (next === 'buckets') await loadBucketsPanel();
    if (next === 'overview' || next === 'recovery') await refreshOverview();
    if (next === 'transfers') await loadTransfers();
    if (next === 'recovery') await loadRecoveryIssues();
    if (next === 'users') await loadUsersPanel();
    if (next === 'telegram') await loadTelegramPanel();
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

  async function selectRecoveryTab(tab: 'issues' | 'transfers') {
    recoveryTab = tab;
    if (tab === 'issues') await loadRecoveryIssues(); else await loadTransfers();
  }

  async function openUploadModal() {
    await Promise.all([loadAdminModals(), loadUploadBox()]);
    showUploadModal = true;
  }

  async function openBucketModal() {
    await loadAdminModals();
    showBucketModal = true;
  }

  async function openFolderModal() {
    await loadAdminModals();
    showFolderModal = true;
  }

  async function openOperatorModal() {
    await loadAdminModals();
    showOperatorModal = true;
  }

  async function openMoveModal() {
    await loadAdminModals();
    moveBucket = selectedBucket;
    movePrefix = currentPrefix;
    showMoveModal = true;
  }

  async function refreshRecovery() {
    recoveryLoading = true;
    try { await refreshOverview({silent: true}); }
    finally { recoveryLoading = false; }
  }

  function toggleKey(key: string) {
    selectedKeys = selectedKeys.includes(key) ? selectedKeys.filter((item) => item !== key) : [...selectedKeys, key];
  }

  async function removeSelected() {
    if (!selectedKeys.length || !confirm(`Delete ${selectedKeys.length} selected item(s)?`)) return;
    busy = true;
    try {
      for (const key of selectedKeys) await removeObject(session?.csrf_token, selectedBucket, key);
      selectedKeys = [];
      message = 'Selected items deleted.';
      await refreshObjects();
    } catch (cause) { error = normalizeError(cause); }
    finally { busy = false; }
  }

  async function moveSelected() {
    if (!selectedKeys.length || !moveBucket) return;
    busy = true;
    try {
      for (const key of selectedKeys) {
        const response = await fetch(contentUrl(selectedBucket, key), { credentials: 'include' });
        if (!response.ok) throw new Error(`Could not read ${key}`);
        const targetKey = `${movePrefix.replace(/^\/+|\/+$/g, '') ? `${movePrefix.replace(/^\/+|\/+$/g, '')}/` : ''}${key.split('/').pop() ?? key}`;
        await uploadObject(moveBucket, targetKey, await response.blob(), session?.csrf_token);
        await removeObject(session?.csrf_token, selectedBucket, key);
      }
      selectedKeys = []; showMoveModal = false; message = 'Selected items moved.'; await refreshObjects();
    } catch (cause) { error = normalizeError(cause); }
    finally { busy = false; }
  }
</script>

<svelte:head>
  <title>Telegram S3 — Management</title>
</svelte:head>

<TopProgress active={anyLoading}/>

<main class="shell" class:signed-in={session?.authenticated}>
  {#if session?.authenticated}
    <Sidebar {view} username={session.user?.username ?? ''} onNavigate={switchView} onLogout={handleLogout}/>
    <header class="console-header"><div><p class="card-label">Workspace</p><h1>{view==='users'?'Operators':view==='telegram'?'Telegram settings':view.charAt(0).toUpperCase()+view.slice(1)}</h1></div><HealthBadge state={overviewLoading || !overview ? 'checking' : overview.telegram?.connection_state ?? 'checking'} detail={overviewLoading || !overview ? 'Checking Telegram connection…' : overview.telegram?.detail ?? 'Waiting for a connection check'} checkedAt={overviewLoading ? undefined : overview?.checked_at}/></header>
  {/if}
  {#if loading}
    <section class="card surface"><p>Loading…</p></section>
  {:else if setupRequired && !session?.authenticated}
    {#if SetupWizardComponent}<svelte:component this={SetupWizardComponent} onCreated={(created: SessionState)=>{session=created;setupRequired=false;view='telegram';void loadTelegramPanel();void refreshOverview();void refreshTelegramSettings();}}/>{:else}<section class="card surface"><div class="skeleton" style="height:240px"></div></section>{/if}
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
      {#if TransfersComponent}<svelte:component this={TransfersComponent} csrf={session?.csrf_token}/>{:else}<section class="card surface"><div class="skeleton" style="height:280px"></div></section>{/if}
    {:else if view === 'recovery'}
      <div class="subtabs" role="tablist" aria-label="Recovery views">
        <button class:active={recoveryTab === 'issues'} role="tab" aria-selected={recoveryTab === 'issues'} on:click={() => void selectRecoveryTab('issues')}>Issues</button>
        <button class:active={recoveryTab === 'transfers'} role="tab" aria-selected={recoveryTab === 'transfers'} on:click={() => void selectRecoveryTab('transfers')}>Interrupted transfers</button>
      </div>
      {#if recoveryTab === 'issues'}
        {#if RecoveryIssuesComponent}<svelte:component this={RecoveryIssuesComponent} recovery={overview?.recovery} csrf={session?.csrf_token} loading={recoveryLoading} onChanged={() => refreshOverview({silent: true})} onRefresh={refreshRecovery}/>{:else}<section class="card surface"><div class="skeleton" style="height:180px"></div></section>{/if}
      {:else}
        {#if TransfersComponent}<svelte:component this={TransfersComponent} csrf={session?.csrf_token} recoveryOnly/>{:else}<section class="card surface"><div class="skeleton" style="height:180px"></div></section>{/if}
      {/if}
    {:else if view === 'telegram'}
      {#if TelegramPanelComponent}<svelte:component this={TelegramPanelComponent} bind:telegramApiId bind:telegramApiHash bind:telegramStorageChatId bind:telegramProxyUrl bind:telegramProxyUsername bind:telegramProxyPassword bind:telegramProxyMode overview={overview} {session} settingsBusy={telegramSettingsBusy} settingsError={telegramSettingsError} settingsMessage={telegramSettingsMessage} bind:showCredentials={showTelegramCredentials} {showWizard} wizardComponent={TelegramWizardComponent} onSave={saveTelegramSettingsForm} onManageOperators={() => switchView('users')} onToggleWizard={toggleWizard} onWizardDone={handleWizardAuthorized}/>{:else}<section class="card surface"><div class="skeleton" style="height:360px"></div></section>{/if}
    {:else if view === 'overview'}
      {#if OverviewPanelComponent}<svelte:component this={OverviewPanelComponent} overview={overview} loading={overviewLoading} {corruptedCount} {acknowledgedCount} onRefresh={() => refreshOverview()} onRecovery={() => switchView('recovery')}/>{:else}<section class="card surface"><div class="skeleton" style="height:280px"></div></section>{/if}
    {:else if view === 'buckets'}
      {#if BucketsPanelComponent}<svelte:component this={BucketsPanelComponent} buckets={buckets} selectedBucket={selectedBucket} currentPrefix={currentPrefix} {listing} {bucketsLoading} {objectsLoading} {busy} {selectedKeys} onCreateBucket={openBucketModal} onRefresh={() => selectedBucket ? refreshObjects() : refreshBuckets()} onUpload={openUploadModal} onOpenBucket={openBucket} onExitBucket={exitBucket} onEnterFolder={enterFolder} onGotoCrumb={gotoCrumb} onOpenFolder={openFolderModal} onToggleKey={toggleKey} onToggleAll={() => selectedKeys = selectedKeys.length ? [] : (listing?.objects.map((obj) => obj.key) ?? [])} onRemoveKey={removeKey} onRemoveSelected={removeSelected} onOpenMove={openMoveModal}/>{:else}<section class="card surface"><div class="skeleton" style="height:280px"></div></section>{/if}
    {:else if view === 'users'}
      {#if UsersPanelComponent}<svelte:component this={UsersPanelComponent} {users} loading={usersLoading} canManage={canManageOperators} {busy} onRefresh={refreshUsers} onRemove={dropUser} onAdd={openOperatorModal}/>{:else}<section class="card surface"><div class="skeleton" style="height:220px"></div></section>{/if}
    {/if}
  {/if}

  {#if AdminModalsComponent}<svelte:component this={AdminModalsComponent} bind:showBucket={showBucketModal} bind:showFolder={showFolderModal} bind:showUpload={showUploadModal} bind:showOperator={showOperatorModal} bind:showMove={showMoveModal} bind:newBucket bind:newFolder bind:newUsername bind:newDisplay bind:newPassword bind:newRole bind:moveBucket bind:movePrefix selectedBucket={selectedBucket} currentPrefix={currentPrefix} {buckets} {busy} uploadComponent={UploadBoxComponent} csrf={session?.csrf_token} onCreateBucket={makeBucket} onCreateFolder={makeFolder} onCreateOperator={makeUser} onMove={moveSelected} onUploaded={refreshObjects}/>{/if}

  {#if message}
    <section class="toast success" role="status">{message}<button class="toast-close" type="button" aria-label="Dismiss notification" on:click={() => message = ''}>×</button></section>
  {/if}
  {#if error}
    <section class="toast error" role="alert">{error}<button class="toast-close" type="button" aria-label="Dismiss notification" on:click={() => error = ''}>×</button></section>
  {/if}
</main>

<style>
  :global {
  .shell.signed-in{width:calc(100vw - 230px);max-width:none;margin:0 0 0 230px;padding:30px 40px;min-height:100vh}
  .console-header{display:flex;justify-content:space-between;gap:24px;align-items:center;margin-bottom:30px}
  .console-header h1{font-size:26px;letter-spacing:-.04em;margin:4px 0}
  .address-root{display:inline-flex;gap:7px;align-items:center}.address-current{padding:.45rem .75rem;border-radius:8px;background:var(--accent-soft);color:var(--accent)}.select-all{width:16px;height:16px;padding:0;accent-color:var(--accent)}.selection-bar{display:flex;gap:8px;align-items:center;padding:10px 0}.danger-button{background:#b33838}.settings-summary{align-items:start}.settings-card{border:1px solid var(--border);border-radius:var(--radius-md);padding:1rem;background:var(--surface)}.proxy-form{display:grid;gap:12px}.modal-backdrop{position:fixed;inset:0;background:rgba(12,25,42,.58);display:grid;place-items:center;padding:20px;z-index:20}.modal-card{width:min(620px,100%);max-height:calc(100vh - 40px);overflow:auto;display:grid;gap:16px;padding:22px;border:1px solid var(--border);border-radius:var(--radius-lg);background:var(--surface);box-shadow:0 20px 60px rgba(13,31,52,.25)}.compact-modal{width:min(430px,100%)}.icon-button{width:36px;height:36px;padding:0;border-radius:50%;background:var(--accent-soft);color:var(--text);font-size:1.35rem}
  .subtabs{display:flex;gap:6px;margin-bottom:16px;padding:4px;border-radius:var(--radius-md);background:color-mix(in srgb,var(--text) 5%,transparent);width:max-content;max-width:100%;overflow:auto}.subtabs button{background:transparent;color:var(--muted);padding:.6rem .9rem;white-space:nowrap}.subtabs button.active{background:var(--surface);color:var(--accent);box-shadow:var(--shadow)}
  .toast-close{margin-left:auto;padding:.1rem .35rem;background:transparent;color:var(--muted);font-size:1.1rem}
  @media(max-width:760px){.shell.signed-in{width:100%;margin-left:0;padding:0 16px 24px}.console-header{margin-top:24px;align-items:flex-start;flex-direction:column}.settings-summary{grid-template-columns:1fr}}

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
  }
</style>
