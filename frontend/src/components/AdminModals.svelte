<script lang="ts">
  import MoveBrowser from './MoveBrowser.svelte';
  export let showBucket = false;
  export let showFolder = false;
  export let showUpload = false;
  export let showOperator = false;
  export let showMove = false;
  export let showShare = false;
  export let showDelete = false;
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
  export let shareTarget: { name: string; key: string; size: number; expires_at?: string | null } | null = null;
  export let shareExpiry = '';
  export let shareUrl = '';
  export let shareError = '';
  export let shareBusy = false;
  export let deleteTarget: { type: string; name: string; key?: string } | null = null;
  export let onCreateShare: () => void = () => {};
  export let onConfirmDelete: () => void = () => {};

  let uploadInProgress = false;
  let confirmUploadClose = false;
  let confirmUploadCloseBusy = false;
  let uploadInstance: { cancelActiveUploads?: () => Promise<void> } | null = null;

  function requestUploadClose() {
    if (uploadInProgress) {
      confirmUploadClose = true;
      return;
    }
    showUpload = false;
  }

  async function cancelUploadAndClose() {
    confirmUploadCloseBusy = true;
    try {
      await uploadInstance?.cancelActiveUploads?.();
      confirmUploadClose = false;
      showUpload = false;
    } finally {
      confirmUploadCloseBusy = false;
    }
  }
</script>

{#if showBucket}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showBucket = false)}><form class="modal-card compact-modal" on:submit|preventDefault={() => { showBucket = false; onCreateBucket(); }}><div class="section-head"><div><p class="card-label">Buckets</p><h2>Create bucket</h2></div><button class="icon-button" type="button" on:click={() => showBucket = false}>×</button></div><label><span>Bucket name</span><input bind:value={newBucket} placeholder="e.g. documents or فایل‌ها" /></label><p class="fine-print">Unicode names are supported and preserved exactly.</p><button class="primary" type="submit" disabled={busy || !newBucket.trim()}>Create bucket</button></form></div>{/if}
{#if showUpload && selectedBucket}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && requestUploadClose()}><div class="modal-card upload-modal"><div class="modal-hero"><div class="modal-icon upload-icon">↑</div><div><p class="card-label">{selectedBucket}</p><h2>Bring files into this folder</h2><p class="modal-subtitle">Choose files, set an optional lifetime, and we’ll keep the upload moving safely in the background.</p></div><button class="icon-button" type="button" aria-label="Close upload dialog" title="Close" on:click={requestUploadClose}>×</button></div>{#if uploadComponent}<svelte:component this={uploadComponent} bind:this={uploadInstance} bucket={selectedBucket} prefix={currentPrefix} {csrf} onUploaded={onUploaded} onUploadActivity={(active: boolean) => uploadInProgress = active}/>{:else}<div class="skeleton" style="height:180px"></div>{/if}</div></div>{/if}
{#if showShare && shareTarget}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && !shareBusy && (showShare = false)}><form class="modal-card compact-modal share-modal" on:submit|preventDefault={onCreateShare}><div class="modal-hero"><div class="modal-icon share-icon">↗</div><div><p class="card-label">Public link</p><h2>Share {shareTarget.name}</h2><p class="modal-subtitle">Create a private-looking download link without exposing your S3 credentials.</p></div><button class="icon-button" type="button" aria-label="Close share dialog" title="Close" on:click={() => showShare = false} disabled={shareBusy}>×</button></div>{#if !shareUrl}<div class="expiry-choice"><label><span>Link lifetime</span><input bind:value={shareExpiry} type="number" min="1" step="1" placeholder="No expiry" /></label><div class="preset-row"><button type="button" class="preset" on:click={() => shareExpiry = '900'}>15 min</button><button type="button" class="preset" on:click={() => shareExpiry = '3600'}>1 hour</button><button type="button" class="preset" on:click={() => shareExpiry = '86400'}>1 day</button><button type="button" class="preset" on:click={() => shareExpiry = ''}>Never</button></div><p class="fine-print">The link can never outlive the object’s own expiry.</p></div>{:else}<div class="share-result"><span class="result-label">Link ready</span><input readonly value={shareUrl} aria-label="Public share URL" on:focus={(event) => event.currentTarget.select()} /><button type="button" class="primary" on:click={() => navigator.clipboard.writeText(shareUrl)}>Copy link</button><p class="fine-print">Anyone with this URL can download the file until it expires.</p></div>{/if}{#if shareError}<p class="error-hint" role="alert">{shareError}</p>{/if}{#if !shareUrl}<div class="settings-actions"><button class="ghost" type="button" on:click={() => showShare = false} disabled={shareBusy}>Cancel</button><button class="primary" type="submit" disabled={shareBusy}>{shareBusy ? 'Creating link…' : 'Create share link'}</button></div>{/if}</form></div>{/if}
{#if showDelete && deleteTarget}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && !busy && (showDelete = false)}><form class="modal-card compact-modal danger-modal" on:submit|preventDefault={onConfirmDelete}><div class="modal-hero"><div class="modal-icon danger-icon">!</div><div><p class="card-label">Destructive action</p><h2>{deleteTarget.type === 'selection' ? 'Delete selected items?' : `Delete ${deleteTarget.type}?`}</h2></div><button class="icon-button" type="button" aria-label="Close delete dialog" title="Close" on:click={() => showDelete = false} disabled={busy}>×</button></div><p>Are you sure you want to delete <strong>{deleteTarget.name}</strong>? This immediately hides the item and schedules safe remote cleanup.</p>{#if deleteTarget.type === 'bucket'}<p class="fine-print">The bucket must already be empty.</p>{/if}<div class="settings-actions"><button class="ghost" type="button" on:click={() => showDelete = false} disabled={busy}>Cancel</button><button class="danger-button" type="submit" disabled={busy}>{busy ? 'Deleting…' : 'Delete'}</button></div></form></div>{/if}
{#if confirmUploadClose}<div class="modal-backdrop upload-confirm-backdrop" role="presentation"><div class="modal-card compact-modal" role="dialog" aria-modal="true" aria-labelledby="upload-close-title"><p class="card-label">Upload still in progress</p><h2 id="upload-close-title">Cancel this upload?</h2><p class="fine-print">The file is still uploading to the server. Closing this window will cancel the upload and its resumable reception.</p><div class="settings-actions"><button class="ghost" type="button" on:click={() => confirmUploadClose = false} disabled={confirmUploadCloseBusy}>Keep uploading</button><button class="danger-button" type="button" on:click={() => void cancelUploadAndClose()} disabled={confirmUploadCloseBusy}>{confirmUploadCloseBusy ? 'Cancelling…' : 'Cancel upload'}</button></div></div></div>{/if}
{#if showFolder && selectedBucket}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showFolder = false)}><form class="modal-card compact-modal" on:submit|preventDefault={() => { showFolder = false; onCreateFolder(); }}><div class="section-head"><div><p class="card-label">{selectedBucket}</p><h2>New folder</h2></div><button class="icon-button" type="button" on:click={() => showFolder = false}>×</button></div><label><span>Folder name</span><input bind:value={newFolder} placeholder="e.g. invoices/2026" /></label><p class="fine-print">Created inside {currentPrefix || 'the bucket root'}.</p><button class="primary" type="submit" disabled={busy || !newFolder.trim()}>Create folder</button></form></div>{/if}
{#if showOperator}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showOperator = false)}><form class="modal-card" on:submit|preventDefault={() => { showOperator = false; onCreateOperator(); }}><div class="section-head"><div><p class="card-label">Operators</p><h2>Add operator</h2></div><button class="icon-button" type="button" on:click={() => showOperator = false}>×</button></div><div class="grid-2"><label><span>Username</span><input bind:value={newUsername} autocomplete="off" /></label><label><span>Display name</span><input bind:value={newDisplay} autocomplete="off" /></label><label><span>Password (12+ chars)</span><input bind:value={newPassword} type="password" autocomplete="new-password" /></label><label><span>Role</span><select bind:value={newRole}><option value="admin">admin</option><option value="superadmin">superadmin</option></select></label></div><button class="primary" type="submit" disabled={busy || !newUsername || !newPassword}>Add operator</button></form></div>{/if}
{#if showMove}<div class="modal-backdrop" role="presentation" on:click={(event) => event.target === event.currentTarget && (showMove = false)}><div class="modal-card"><div class="section-head"><div><p class="card-label">Move selected items</p><h2>Choose destination</h2></div><button class="icon-button" type="button" on:click={() => showMove = false} disabled={busy}>×</button></div><MoveBrowser bind:bucket={moveBucket} bind:prefix={movePrefix} {csrf} {busy} onMoveHere={onMove}/><p class="fine-print">Files are copied, then removed from the current bucket.</p></div></div>{/if}

<style>
  .upload-confirm-backdrop { z-index: 25; }
  .modal-hero { display:grid; grid-template-columns:auto minmax(0,1fr) auto; gap:14px; align-items:start; }
  .modal-hero h2 { margin:.25rem 0 .35rem; letter-spacing:-.04em; }
  .modal-subtitle { margin:0; color:var(--muted); line-height:1.5; font-size:.9rem; }
  .modal-icon { display:grid; place-items:center; width:46px; height:46px; border-radius:15px; font-size:1.35rem; font-weight:800; }
  .upload-icon { background:linear-gradient(135deg,#dceeff,#edf7ff); color:#216dba; }
  .share-icon { background:linear-gradient(135deg,#e1f6ed,#effbf5); color:#167858; }
  .danger-icon { background:#fff0f0; color:var(--danger); }
  .upload-modal { width:min(720px,100%); }
  .share-modal { width:min(500px,100%); }
  .expiry-choice,.share-result { display:grid; gap:10px; padding:16px; border:1px solid var(--border); border-radius:16px; background:linear-gradient(135deg,#fbfdff,#f5f9fc); }
  .expiry-choice label { display:grid; gap:6px; }
  .preset-row { display:flex; flex-wrap:wrap; gap:7px; }
  .preset { min-height:36px; padding:.45rem .7rem; border:1px solid var(--border); background:var(--surface); color:var(--text); font-size:.8rem; }
  .preset:hover:not(:disabled) { background:var(--accent-soft); color:var(--accent); border-color:var(--accent-ring); }
  .result-label { color:var(--ok); font-size:.78rem; font-weight:800; text-transform:uppercase; letter-spacing:.08em; }
  .danger-modal { border-color:#f0cccc; }
  @media(max-width:520px) { .modal-hero { grid-template-columns:auto minmax(0,1fr); } .modal-hero > .icon-button { grid-column:2; grid-row:1; justify-self:end; } }
</style>
