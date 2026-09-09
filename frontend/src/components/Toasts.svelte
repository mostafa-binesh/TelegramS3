<script lang="ts">
  import { flip } from 'svelte/animate';
  import { fly } from 'svelte/transition';
  import { dismissToast, reducedMotion, toasts } from '../lib/toasts';

  // Honour the OS motion preference by collapsing the animation to nothing
  // rather than by dropping the transition, so nodes are still removed cleanly.
  const duration = reducedMotion() ? 0 : 220;
</script>

{#if $toasts.length}
  <div class="toast-stack">
    {#each $toasts as toast (toast.id)}
      <section
        class="toast {toast.kind}"
        role={toast.kind === 'error' ? 'alert' : 'status'}
        in:fly={{ y: 14, duration }}
        out:fly={{ y: 8, duration: Math.round(duration * 0.8) }}
        animate:flip={{ duration }}
      >
        <span class="toast-text">{toast.text}</span>
        <button
          class="toast-close"
          type="button"
          aria-label="Dismiss notification"
          on:click={() => dismissToast(toast.id)}>×</button
        >
      </section>
    {/each}
  </div>
{/if}

<style>
  .toast-text {
    word-break: break-word;
  }
</style>
