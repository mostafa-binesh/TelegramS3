<script lang="ts">
  export let name: 'download' | 'share' | 'trash' | 'refresh' | 'folder' | 'retry' | 'cancel' | 'copy' | 'close' | 'back';
  export let label: string;
  export let disabled = false;
  export let tone: 'default' | 'danger' = 'default';
  export let href: string | null = null;
</script>

{#if href}<a class:danger={tone === 'danger'} class="action-icon" {href} download aria-label={label} title={label}>
  <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12m0 0 4-4m-4 4-4-4M5 21h14" /></svg>
</a>{:else}<button class:danger={tone === 'danger'} class="action-icon" type="button" {disabled} aria-label={label} title={label} on:click>
  <svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
    {#if name === 'download'}<path d="M12 3v12m0 0 4-4m-4 4-4-4M5 21h14" />
    {:else if name === 'share'}<circle cx="18" cy="5" r="2.5" /><circle cx="6" cy="12" r="2.5" /><circle cx="18" cy="19" r="2.5" /><path d="m8.2 10.8 7.6-4.5m-7.6 6.9 7.6 4.5" />
    {:else if name === 'trash'}<path d="M4 7h16m-10 4v6m4-6v6M9 7V4h6v3m-9 0 1 13h10l1-13" />
    {:else if name === 'refresh'}<path d="M20 11a8 8 0 0 0-14.8-3L3 11m0 0V5m0 6h6M4 13a8 8 0 0 0 14.8 3L21 13m0 0v6m0-6h-6" />
    {:else if name === 'folder'}<path d="M3.5 7.5h6l2 2h9l-1.2 9H4.7l-1.2-11Z" /><path d="M3.5 7.5V5.8h6l2 2" />
    {:else if name === 'retry'}<path d="M20 11a8 8 0 0 0-14.6-3L3 11m0 0V5m0 6h6M4 13a8 8 0 0 0 14.6 3L21 13" />
    {:else if name === 'cancel'}<circle cx="12" cy="12" r="8.5" /><path d="m9 9 6 6m0-6-6 6" />
    {:else if name === 'copy'}<rect x="8" y="8" width="11" height="11" rx="2" /><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
    {:else if name === 'close'}<path d="m6 6 12 12M18 6 6 18" />
    {:else}<path d="m15 5-7 7 7 7" />{/if}
  </svg>
</button>{/if}

<style>
  .action-icon { width: 38px; min-height: 38px; height: 38px; padding: 0; border: 1px solid var(--border); border-radius: 11px; background: var(--surface); color: var(--muted); text-decoration: none; display: inline-flex; align-items: center; justify-content: center; }
  .action-icon:hover:not(:disabled) { color: var(--accent); background: var(--accent-soft); border-color: var(--accent-ring); }
  .action-icon.danger:hover:not(:disabled) { color: var(--danger); background: color-mix(in srgb, var(--danger) 8%, transparent); border-color: color-mix(in srgb, var(--danger) 30%, var(--border)); }
  svg { width: 18px; height: 18px; }
</style>
