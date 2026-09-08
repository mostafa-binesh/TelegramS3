<script lang="ts">
  import { setupAccount } from '../lib/api';
  import type {SessionState} from '../lib/types';
  export let onCreated: (session:SessionState)=>void;
  let username=''; let password=''; let confirm=''; let error=''; let busy=false;
  async function create(){
    error=''; if(password!==confirm){error='Passwords do not match.';return;}
    busy=true;
    try{onCreated(await setupAccount(username,password));}catch(e){error=e instanceof Error?e.message:'Account creation failed';}finally{busy=false;}
  }
</script>
<section class="setup card surface">
  <p class="card-label">Welcome to Telegram S3</p><h1>Create your operator account</h1>
  <p>This first account manages operators and storage. After creating it, connect your Telegram storage account.</p>
  <form on:submit|preventDefault={create}>
    <label>Username<input required bind:value={username} autocomplete="username" maxlength="64" /></label>
    <label>Password<input required bind:value={password} type="password" autocomplete="new-password" minlength="12" /></label>
    <label>Confirm password<input required bind:value={confirm} type="password" autocomplete="new-password" minlength="12" /></label>
    <small>Use at least 12 characters.</small>
    {#if error}<p role="alert" class="error-hint">{error}</p>{/if}
    <button class="primary" disabled={busy}>{busy?'Creating account…':'Create superadmin'}</button>
  </form>
</section>
<style>.setup{max-width:530px;margin:8vh auto;padding:36px}h1{font-size:28px}form{display:grid;gap:18px;margin-top:24px}label{display:grid;gap:8px;font-size:14px}p,small{color:var(--muted);line-height:1.6}</style>
