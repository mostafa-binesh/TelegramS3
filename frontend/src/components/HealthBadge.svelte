<script lang="ts">
  export let state = 'checking';
  export let detail = 'Waiting for a connection check';
  export let checkedAt: string | undefined = undefined;
  export let accounts: Array<{id: string; label: string; connected: boolean}> = [];
</script>
<div class="health" title={detail} role="status">
  <span class="dot" class:connected={state === 'connected'} class:partial={state === 'partial'} class:checking={state === 'checking'}></span>
  <div><strong>Telegram · {state === 'checking' ? 'checking connection' : state.replaceAll('_', ' ')}</strong><small>{detail}</small>
    {#if checkedAt}<small>Checked {new Date(checkedAt).toLocaleTimeString()}</small>{/if}
    {#if accounts.length}<div class="account-dots" aria-label="Telegram account connection status">{#each accounts as account (account.id)}<span class="account-dot" class:connected={account.connected} class:disconnected={!account.connected} title={account.label} aria-label={`${account.label}: ${account.connected ? 'connected' : 'disconnected'}`}></span>{/each}</div>{/if}
  </div>
</div>
<style>
  .health{display:flex;gap:.65rem;align-items:flex-start;font-size:.8rem}.dot{width:9px;height:9px;border-radius:50%;background:#c64747;flex-shrink:0;margin-top:5px}.dot.connected,.account-dot.connected{background:#168466}.dot.partial{background:#c48a2d}.dot.checking{animation:pulse 900ms ease-in-out infinite alternate}small{display:block;color:var(--muted);font-size:.72rem;margin-top:.25rem;max-width:32rem}strong{font-weight:600}.account-dots{display:flex;gap:5px;margin-top:7px}.account-dot{width:8px;height:8px;border-radius:50%;background:#c64747;box-shadow:0 0 0 2px rgba(255,255,255,.7)}@keyframes pulse{from{opacity:.35}to{opacity:1}}
</style>
