<script lang="ts">
  export let view: string;
  export let username: string;
  export let onNavigate: (view: 'overview'|'buckets'|'transfers'|'recovery'|'telegram'|'users') => void;
  export let onLogout: () => void;
  const links = [
    ['overview','Overview','⌂'],['buckets','Buckets','▦'],['transfers','Transfers','↕'],
    ['recovery','Recovery','⟳'],['telegram','Telegram settings','⚙'],['users','Operators','♙']
  ] as const;
</script>
<aside>
  <a class="brand" href="/_admin"><span class="brand-mark">T</span><span>Telegram S3<small>Storage console</small></span></a>
  <nav aria-label="Main navigation">{#each links as [id,label,icon]}<button class:chosen={view===id} aria-current={view===id?'page':undefined} on:click={()=>onNavigate(id)}><span class="nav-icon" aria-hidden="true">{icon}</span><span>{label}</span></button>{/each}</nav>
  <footer><strong>{username}</strong><button on:click={onLogout}>Sign out</button></footer>
</aside>
<style>
  aside{position:fixed;inset:0 auto 0 0;width:230px;background:#142235;color:#e8eef6;padding:30px 18px;display:flex;flex-direction:column;z-index:5}.brand{display:flex;gap:12px;align-items:center;font-size:28px;color:white;text-decoration:none;padding:0 12px 34px}.brand>span:last-child{font-size:16px;font-weight:650}.brand small{display:block;font-size:11px;color:#9aacbf;margin-top:4px;font-weight:400}.brand-mark{display:grid;place-items:center;width:34px;height:34px;border-radius:10px;background:#2d78bd;color:#fff;font-weight:800}nav{display:grid;gap:5px}nav button{width:100%;text-align:left;background:transparent;color:#c8d3e0;border-radius:8px;padding:12px 15px;font-size:14px;justify-content:flex-start}.nav-icon{width:22px;text-align:center;font-size:18px;line-height:1}.chosen{background:#29425e;color:white}footer{margin-top:auto;display:grid;gap:10px;font-size:13px;padding:14px}footer button{padding:8px 0;justify-content:flex-start} @media(max-width:760px){aside{position:static;width:100%;padding:16px}.brand{padding-bottom:14px}nav{grid-template-columns:repeat(3,1fr);gap:3px}nav button{justify-content:center;text-align:center;flex-direction:column;padding:10px 3px;font-size:12px}.nav-icon{font-size:16px}footer{display:flex;justify-content:space-between;padding-bottom:0}}
</style>
