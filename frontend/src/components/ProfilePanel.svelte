<script lang="ts">
  import type { SessionState } from '../lib/types';
  import LoadError from './LoadError.svelte';

  export let session: SessionState | null = null;
  export let busy = false;
  export let error = '';
  export let message = '';
  export let onSave: (body: { display_name: string; password?: string }) => Promise<void> = async () => {};

  let displayName = '';
  let newPassword = '';
  let confirmPassword = '';
  let validationError = '';
  let initializedFor = '';

  $: if (session?.user && initializedFor !== session.user.id) {
    initializedFor = session.user.id;
    displayName = session.user.display_name || session.user.username;
    newPassword = '';
    confirmPassword = '';
    validationError = '';
  }

  async function save() {
    validationError = '';
    const normalizedName = displayName.trim();
    if (!normalizedName) {
      validationError = 'Enter a display name.';
      return;
    }
    if (normalizedName.length > 120) {
      validationError = 'Display name must be 120 characters or fewer.';
      return;
    }
    if (newPassword && newPassword.length < 12) {
      validationError = 'New password must be at least 12 characters.';
      return;
    }
    if (newPassword !== confirmPassword) {
      validationError = 'New password and confirmation do not match.';
      return;
    }
    await onSave(newPassword ? { display_name: normalizedName, password: newPassword } : { display_name: normalizedName });
    newPassword = '';
    confirmPassword = '';
  }
</script>

<section class="profile-layout">
  <div class="card surface profile-hero">
    <div class="hero-icon" aria-hidden="true">{(displayName || session?.user?.username || 'A').slice(0, 1).toUpperCase()}</div>
    <div><p class="card-label">Operator profile</p><h2>{displayName || session?.user?.username || 'Your profile'}</h2><p class="fine-print">Manage the name shown in the console and rotate your password without leaving the current session.</p></div>
    <span class="role-pill">{session?.user?.role ?? 'operator'}</span>
  </div>

  <form class="card surface profile-form" on:submit|preventDefault={save}>
    <div class="section-head"><div><p class="card-label">Profile details</p><h2>Personalize your workspace</h2><p class="fine-print">Your username stays fixed for sign-in. The display name is shown in the navigation and operator surfaces.</p></div><span class="profile-username">@{session?.user?.username ?? ''}</span></div>
    {#if error}<LoadError title="Could not save your profile" message={error} />{/if}
    {#if validationError}<p class="form-message error" role="alert">{validationError}</p>{/if}
    {#if message}<p class="form-message success" role="status">{message}</p>{/if}
    <label><span>Display name</span><input aria-label="Display name" bind:value={displayName} maxlength="120" autocomplete="name" /></label>
    <div class="password-heading"><div><p class="card-label">Security</p><h3>Change password</h3></div><span class="fine-print">Optional</span></div>
    <p class="fine-print password-note">Leave both password fields blank to keep the current password. New passwords must be at least 12 characters.</p>
    <div class="form-grid">
      <label><span>New password</span><input aria-label="New password" bind:value={newPassword} type="password" autocomplete="new-password" /></label>
      <label><span>Confirm new password</span><input aria-label="Confirm new password" bind:value={confirmPassword} type="password" autocomplete="new-password" /></label>
    </div>
    <div class="form-actions"><span class="fine-print">Password changes sign out other active sessions.</span><button class="primary" type="submit" disabled={busy}>{busy ? 'Saving…' : 'Save profile'}</button></div>
  </form>
</section>

<style>
  .profile-layout { display: grid; gap: 18px; }
  .profile-hero { display: flex; align-items: center; gap: 18px; padding: 24px 26px; }
  .hero-icon { display: grid; place-items: center; width: 64px; height: 64px; flex: 0 0 auto; border-radius: 20px; background: linear-gradient(135deg, #74d4c1, #3984c3); color: #12314d; font-size: 28px; font-weight: 850; box-shadow: 0 12px 24px rgba(24, 89, 139, .16); }
  .profile-hero h2 { margin: 3px 0 5px; }
  .profile-hero .fine-print { margin: 0; max-width: 680px; }
  .role-pill { margin-left: auto; align-self: flex-start; padding: 7px 11px; border-radius: 999px; color: var(--accent); background: var(--accent-soft); font-size: 11px; font-weight: 800; text-transform: uppercase; letter-spacing: .07em; }
  .profile-form { padding: 26px; display: grid; gap: 16px; }
  .profile-form h2, .profile-form h3 { margin: 3px 0 4px; }
  .profile-form .section-head { align-items: flex-start; }
  .profile-username { color: var(--muted); font-size: 14px; white-space: nowrap; }
  .password-heading { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding-top: 8px; border-top: 1px solid var(--border); }
  .password-note { margin: -8px 0 0; }
  .form-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px; }
  .form-actions { display: flex; align-items: center; justify-content: space-between; gap: 18px; padding-top: 4px; }
  .form-message { margin: 0; padding: 10px 12px; border-radius: 10px; font-size: 14px; }
  .form-message.error { color: #a52c2c; background: #fff0f0; border: 1px solid #f1c3c3; }
  .form-message.success { color: #16785d; background: #eaf9f3; border: 1px solid #b7e7d6; }
  @media (max-width: 640px) { .profile-hero { align-items: flex-start; flex-wrap: wrap; } .role-pill { margin-left: 0; } .form-grid { grid-template-columns: 1fr; } .form-actions { align-items: stretch; flex-direction: column; } .form-actions button { width: 100%; } }
</style>
