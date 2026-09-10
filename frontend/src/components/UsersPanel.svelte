<script lang="ts">
  import LoadError from './LoadError.svelte';
  import ActionIcon from './ActionIcon.svelte';
  import type { UserInfo } from '../lib/types';
  export let users: UserInfo[] = [];
  export let loading = false;
  export let error = '';
  export let canManage = false;
  export let busy = false;
  export let onRefresh: () => void = () => {};
  export let onRemove: (id: string) => void = () => {};
  export let onAdd: () => void = () => {};
</script>

<section class="card surface">
  <div class="section-head"><div><p class="card-label">Operators</p><h2>Accounts</h2><p class="fine-print">Dashboard operator accounts. Telegram storage access is managed separately.</p></div>
    <button class="ghost" type="button" on:click={onRefresh} disabled={loading}>{#if loading}<span class="spinner" aria-hidden="true"></span>{/if}Refresh</button>
  </div>
  {#if error}<LoadError title="Could not load operator accounts" message={error} onRetry={onRefresh} />{/if}
  {#if loading && users.length === 0 && !error}
    <div class="skeleton-stack"><div class="skeleton" style="height:44px"></div><div class="skeleton" style="height:44px"></div></div>
  {:else if users.length === 0 && !error}
    <p class="empty-state"><span class="empty-mark" aria-hidden="true">+</span>No operator accounts yet.</p>
  {:else}
    <div class="table-scroll"><table class="kv-table"><thead><tr><th>Username</th><th>Role</th><th>State</th><th></th></tr></thead><tbody>
    {#each users as user (user.id)}<tr><td>{user.username}{#if user.display_name} <small>({user.display_name})</small>{/if}</td><td><span class="role-tag" class:role-super={user.role === 'superadmin'}>{user.role}</span></td><td>{user.disabled ? 'disabled' : 'enabled'}</td><td class="row-actions">{#if canManage}<ActionIcon name="trash" label={`Remove operator ${user.username}`} tone="danger" on:click={() => onRemove(user.id)} disabled={busy}/>{/if}</td></tr>{/each}
    </tbody></table></div>
  {/if}
  <div class="nested-form operator-actions">{#if canManage}<button class="primary" on:click={onAdd}>＋ Add operator</button>{:else}<p class="fine-print">Only superadmins can add or remove operator accounts.</p>{/if}</div>
</section>

<style>
  .table-scroll { overflow-x: auto; }
  .kv-table { width: 100%; border-collapse: collapse; margin-top: 8px; }
  .kv-table th { font-size: 11px; text-transform: uppercase; letter-spacing: .08em; color: var(--muted); }
  .kv-table th, .kv-table td { text-align: left; padding: 12px 10px; border-bottom: 1px solid var(--border); }
  .role-tag { display: inline-block; padding: .15rem .55rem; border-radius: 999px; font-size: .8rem; background: color-mix(in srgb, var(--text) 8%, transparent); color: var(--muted); }
  .role-super { background: var(--accent-soft); color: var(--accent); font-weight: 600; }
  .operator-actions { display: flex; justify-content: flex-end; }
  .nested-form { margin-top: 16px; padding-top: 16px; border-top: 1px solid var(--border); }
</style>
