<script lang="ts">
  export let view: string;
  export let username: string;
  export let onNavigate: (view: 'overview'|'buckets'|'transfers'|'recovery'|'telegram'|'users') => void;
  export let onLogout: () => void;
  const links = [
    ['overview','Overview'],['buckets','Buckets'],['transfers','Transfers'],
    ['recovery','Recovery'],['telegram','Telegram settings'],['users','Operators']
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
    {:else}<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="9" cy="8" r="3"/><path d="M3.5 19a5.5 5.5 0 0 1 11 0"/><path d="M16 11a3 3 0 0 1 4.5 2.6M16.5 19a5 5 0 0 1 4 0"/></svg>{/if}
  </span><span>{label}</span></button>{/each}</nav>
  <footer><strong>{username}</strong><button on:click={onLogout}>Sign out</button></footer>
</aside>
<style>
  aside{position:fixed;inset:0 auto 0 0;width:230px;background:#142235;color:#e8eef6;padding:30px 18px;display:flex;flex-direction:column;z-index:5}.brand{display:flex;gap:12px;align-items:center;font-size:28px;color:white;text-decoration:none;padding:0 12px 34px}.brand>span:last-child{font-size:16px;font-weight:650}.brand small{display:block;font-size:11px;color:#9aacbf;margin-top:4px;font-weight:400}.brand-mark{display:grid;place-items:center;width:34px;height:34px;border-radius:10px;background:#2d78bd;color:#fff;font-weight:800}nav{display:grid;gap:5px}nav button{width:100%;text-align:left;background:transparent;color:#c8d3e0;border-radius:8px;padding:12px 15px;font-size:14px;justify-content:flex-start}.nav-icon{width:22px;height:22px;display:grid;place-items:center;line-height:1}.nav-icon svg{width:18px;height:18px;display:block}.chosen{background:#29425e;color:white}footer{margin-top:auto;display:grid;gap:10px;font-size:13px;padding:14px}footer button{padding:8px 0;justify-content:flex-start} @media(max-width:760px){aside{position:static;width:100%;padding:16px}.brand{padding-bottom:14px}nav{grid-template-columns:repeat(3,1fr);gap:3px}nav button{justify-content:center;text-align:center;flex-direction:column;padding:10px 3px;font-size:12px}.nav-icon{font-size:16px}footer{display:flex;justify-content:space-between;padding-bottom:0}}
</style>
