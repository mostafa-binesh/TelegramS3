<script lang="ts">
  export let title = 'Could not load this data';
  export let message = 'Something went wrong while loading this section.';
  export let onRetry: () => void = () => {};
</script>

<div class="load-error" role="alert">
  <div class="load-error-icon" aria-hidden="true">!</div>
  <div class="load-error-body">
    <strong>{title}</strong>
    <p>{message}</p>
    <button class="ghost" type="button" on:click={onRetry}>Try again</button>
  </div>
</div>

<style>
  .load-error {
    display: flex;
    align-items: flex-start;
    gap: 0.85rem;
    margin-top: 1rem;
    padding: 1rem 1.1rem;
    border: 1px solid color-mix(in srgb, var(--danger, #b00020) 28%, var(--border));
    border-radius: var(--radius-md);
    background: linear-gradient(135deg, color-mix(in srgb, var(--danger, #b00020) 8%, var(--surface)), var(--surface));
  }
  .load-error-icon {
    display: grid;
    flex: 0 0 auto;
    place-items: center;
    width: 2rem;
    height: 2rem;
    border-radius: 50%;
    background: color-mix(in srgb, var(--danger, #b00020) 14%, var(--surface));
    color: var(--danger, #b00020);
    font-weight: 800;
  }
  .load-error-body {
    min-width: 0;
  }
  .load-error-body strong {
    display: block;
    color: var(--text);
  }
  .load-error-body p {
    margin: 0.3rem 0 0.75rem;
    color: var(--muted);
    overflow-wrap: anywhere;
  }
</style>
