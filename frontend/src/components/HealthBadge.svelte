<script lang="ts">
  export let state = 'checking';
  export let detail = 'Waiting for a connection check';
  export let checkedAt: string | undefined = undefined;
</script>
<div class="health" title={detail} role="status">
  <span class="dot" class:connected={state === 'connected'} class:checking={state === 'checking'}></span>
  <div><strong>Telegram · {state === 'checking' ? 'checking connection' : state.replaceAll('_', ' ')}</strong><small>{detail}</small>
    {#if checkedAt}<small>Checked {new Date(checkedAt).toLocaleTimeString()}</small>{/if}
  </div>
</div>
<style>
  .health{display:flex;gap:.65rem;align-items:flex-start;font-size:.8rem}.dot{width:9px;height:9px;border-radius:50%;background:#b7791f;flex-shrink:0;margin-top:5px}.dot.connected{background:#168466}.dot.checking{animation:pulse 900ms ease-in-out infinite alternate}small{display:block;color:var(--muted);font-size:.72rem;margin-top:.25rem;max-width:32rem}strong{font-weight:600}@keyframes pulse{from{opacity:.35}to{opacity:1}}
</style>
