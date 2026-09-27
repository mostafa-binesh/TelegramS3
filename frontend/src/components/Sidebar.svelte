<script lang="ts">
  export let view: string;
  export let username: string;
  export let onNavigate: (view: 'overview'|'buckets'|'transfers'|'recovery'|'telegram'|'accounts'|'users') => void;
  export let onLogout: () => void;
  export let busy = false;
  const links = [
    ['overview','Overview'],['buckets','Buckets'],['transfers','Transfers'],
    ['recovery','Recovery'],['telegram','Storage settings'],['accounts','Accounts'],['users','Operators']
  ] as const;
</script>
<aside>
  <a class="brand" href="/_admin"><span class="brand-mark">T</span><span>Telegram S3<small>Storage console</small></span></a>
  <nav aria-label="Main navigation">{#each links as [id,label]}<button class:chosen={view===id} aria-current={view===id?'page':undefined} on:click={()=>onNavigate(id)}><span class="nav-icon" aria-hidden="true">
    {#if id === 'overview'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><rect x="14" y="14" width="6" height="6" rx="1"/></svg>
    {:else if id === 'buckets'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="m4 7 8-3 8 3-8 3-8-3Z"/><path d="m4 12 8 3 8-3"/><path d="m4 17 8 3 8-3"/></svg>
    {:else if id === 'transfers'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M7 4v15"/><path d="m3 15 4 4 4-4"/><path d="M17 20V5"/><path d="m13 9 4-4 4 4"/></svg>
    {:else if id === 'recovery'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 12a8 8 0 1 0 2.3-5.7"/><path d="M4 5v5h5"/></svg>
    {:else if id === 'telegram'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M4 7h16M4 17h16"/><circle cx="9" cy="7" r="2" fill="currentColor" stroke="none"/><circle cx="15" cy="17" r="2" fill="currentColor" stroke="none"/></svg>
    {:else if id === 'accounts'}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="9" cy="8" r="3"/><path d="M3.5 19a5.5 5.5 0 0 1 11 0"/><path d="M16 11a3 3 0 0 1 4.5 2.6M16.5 19a5 5 0 0 1 4 0"/></svg>
    {:else}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="9" cy="8" r="3"/><path d="M3.5 19a5.5 5.5 0 0 1 11 0"/><path d="M16 11a3 3 0 0 1 4.5 2.6M16.5 19a5 5 0 0 1 4 0"/></svg>{/if}
  </span><span>{label}</span></button>{/each}</nav>
  <footer><div class="account-card"><span class="account-avatar" aria-hidden="true">{username.slice(0, 1).toUpperCase() || 'A'}</span><div class="account-copy"><span class="account-kicker">Signed in as</span><strong>{username}</strong></div></div><button class="sign-out" type="button" on:click={onLogout} disabled={busy}><svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M10 5H6.5A1.5 1.5 0 0 0 5 6.5v11A1.5 1.5 0 0 0 6.5 19H10"/><path d="M13 8l4 4-4 4M9 12h8"/></svg><span>{busy ? 'Signing out…' : 'Sign out'}</span></button></footer>
</aside>
<style>
  aside { position: fixed; inset: 0 auto 0 0; width: 230px; background: #142235; color: #e8eef6; padding: 30px 18px; display: flex; flex-direction: column; z-index: 5; }
  .brand { display: flex; gap: 12px; align-items: center; font-size: 28px; color: white; text-decoration: none; padding: 0 12px 34px; }
  .brand > span:last-child { font-size: 16px; font-weight: 650; }
  .brand small { display: block; font-size: 11px; color: #9aacbf; margin-top: 4px; font-weight: 400; }
  .brand-mark { display: grid; place-items: center; width: 34px; height: 34px; border-radius: 10px; background: #2d78bd; color: #fff; font-weight: 800; }
  nav { display: grid; gap: 5px; }
  nav button { width: 100%; min-width: 0; text-align: left; background: transparent; color: #c8d3e0; border-radius: 8px; padding: 12px 15px; font-size: 14px; justify-content: flex-start; }
  .nav-icon { width: 22px; height: 22px; display: grid; place-items: center; line-height: 1; }
  .nav-icon svg { width: 18px; height: 18px; display: block; }
  .chosen { background: #29425e; color: white; }
  footer { margin-top: auto; display: grid; gap: 12px; font-size: 13px; padding: 18px 2px 2px; border-top: 1px solid rgba(178, 207, 232, .14); }
  .account-card { display: flex; align-items: center; gap: 10px; min-width: 0; padding: 11px 10px; border: 1px solid rgba(178, 207, 232, .14); border-radius: 15px; background: linear-gradient(135deg, rgba(49, 83, 119, .62), rgba(27, 48, 73, .56)); box-shadow: 0 10px 24px rgba(3, 12, 25, .16); }
  .account-avatar { display: grid; place-items: center; flex: 0 0 auto; width: 36px; height: 36px; border-radius: 12px; background: linear-gradient(135deg, #75d2c4, #3e86c3); color: #12314d; font-size: 15px; font-weight: 850; }
  .account-copy { display: grid; gap: 3px; min-width: 0; }
  .account-kicker { color: #9fb5ca; font-size: 9px; font-weight: 800; letter-spacing: .12em; text-transform: uppercase; }
  .account-copy strong { overflow-wrap: anywhere; color: #fff; font-size: 14px; font-weight: 750; }
  .sign-out { width: 100%; min-height: 42px; padding: 0 12px; gap: 8px; background: rgba(179, 56, 56, .08); color: #ffd6d6; border: 1px solid rgba(255, 210, 210, .28); justify-content: center; }
  .sign-out svg { width: 17px; height: 17px; }
  .sign-out:hover:not(:disabled) { background: rgba(179, 56, 56, .24); border-color: #e38c8c; }
  @media(max-width:900px) { aside { position: static; width: 100%; padding: 14px clamp(12px,3vw,24px); } .brand { padding: 0 0 14px; } .brand > span:last-child { font-size: 15px; } nav { grid-template-columns: repeat(3,minmax(0,1fr)); gap: 4px; } nav button { justify-content: center; text-align: center; flex-direction: column; padding: 10px 4px; font-size: 12px; line-height: 1.2; } .nav-icon { font-size: 16px; } footer { display: flex; justify-content: space-between; align-items: center; padding: 12px 0 0; } .account-card { flex: 1 1 auto; padding: 0; border: 0; background: transparent; box-shadow: none; } .sign-out { width: auto; min-width: 112px; } }
  @media(max-width:380px) { .brand { gap: 8px; } .brand-mark { width: 32px; height: 32px; } .brand > span:last-child { font-size: 14px; } .brand small { font-size: 10px; } nav button { font-size: 11px; padding-right: 2px; padding-left: 2px; } }
</style>
