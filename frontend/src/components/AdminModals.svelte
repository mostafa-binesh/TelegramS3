<script lang="ts">
  import MoveBrowser from './MoveBrowser.svelte';
  export let showBucket = false;
  export let showFolder = false;
  export let showUpload = false;
  export let showOperator = false;
  export let showMove = false;
  export let selectedBucket = '';
  export let currentPrefix = '';
  export let newBucket = '';
  export let newFolder = '';
  export let newUsername = '';
  export let newDisplay = '';
  export let newPassword = '';
  export let newRole = 'admin';
  export let moveBucket = '';
  export let movePrefix = '';
  export let busy = false;
  export let uploadComponent: any = null;
  export let csrf: string | null | undefined;
  export let onCreateBucket: () => void = () => {};
  export let onCreateFolder: () => void = () => {};
  export let onCreateOperator: () => void = () => {};
  export let onMove: () => void = () => {};
  export let onUploaded: () => void = () => {};
</script>

{#if showBucket}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showBucket = false)}><form class="modal-card compact-modal" on:submit|preventDefault={() => { showBucket = false; onCreateBucket(); }}><div class="section-head"><div><p class="card-label">Buckets</p><h2>Create bucket</h2></div><button class="icon-button" type="button" on:click={() => showBucket = false}>×</button></div><label><span>Bucket name</span><input bind:value={newBucket} placeholder="e.g. documents or فایل‌ها" /></label><p class="fine-print">Unicode names are supported and preserved exactly.</p><button class="primary" type="submit" disabled={busy || !newBucket.trim()}>Create bucket</button></form></div>{/if}
{#if showUpload && selectedBucket}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showUpload = false)}><div class="modal-card"><div class="section-head"><div><p class="card-label">{selectedBucket}</p><h2>Upload files</h2></div><button class="icon-button" type="button" on:click={() => showUpload = false}>×</button></div>{#if uploadComponent}<svelte:component this={uploadComponent} bucket={selectedBucket} prefix={currentPrefix} {csrf} onUploaded={onUploaded}/>{:else}<div class="skeleton" style="height:180px"></div>{/if}</div></div>{/if}
{#if showFolder && selectedBucket}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showFolder = false)}><form class="modal-card compact-modal" on:submit|preventDefault={() => { showFolder = false; onCreateFolder(); }}><div class="section-head"><div><p class="card-label">{selectedBucket}</p><h2>New folder</h2></div><button class="icon-button" type="button" on:click={() => showFolder = false}>×</button></div><label><span>Folder name</span><input bind:value={newFolder} placeholder="e.g. invoices/2026" /></label><p class="fine-print">Created inside {currentPrefix || 'the bucket root'}.</p><button class="primary" type="submit" disabled={busy || !newFolder.trim()}>Create folder</button></form></div>{/if}
{#if showOperator}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showOperator = false)}><form class="modal-card" on:submit|preventDefault={() => { showOperator = false; onCreateOperator(); }}><div class="section-head"><div><p class="card-label">Operators</p><h2>Add operator</h2></div><button class="icon-button" type="button" on:click={() => showOperator = false}>×</button></div><div class="grid-2"><label><span>Username</span><input bind:value={newUsername} autocomplete="off" /></label><label><span>Display name</span><input bind:value={newDisplay} autocomplete="off" /></label><label><span>Password (12+ chars)</span><input bind:value={newPassword} type="password" autocomplete="new-password" /></label><label><span>Role</span><select bind:value={newRole}><option value="admin">admin</option><option value="superadmin">superadmin</option></select></label></div><button class="primary" type="submit" disabled={busy || !newUsername || !newPassword}>Add operator</button></form></div>{/if}
{#if showMove}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showMove = false)}><div class="modal-card"><div class="section-head"><div><p class="card-label">Move selected items</p><h2>Choose destination</h2></div><button class="icon-button" type="button" on:click={() => showMove = false} disabled={busy}>×</button></div><MoveBrowser bind:bucket={moveBucket} bind:prefix={movePrefix} {csrf} {busy} onMoveHere={onMove}/><p class="fine-print">Files are copied, then removed from the current bucket.</p></div></div>{/if}
