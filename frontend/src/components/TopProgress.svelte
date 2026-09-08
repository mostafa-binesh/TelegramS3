<script lang="ts">
  /** Shown only for operator-initiated loads; the background poll stays silent. */
  export let active = false;
</script>

<div class="top-progress" class:active aria-hidden="true">
  <div class="bar"></div>
</div>

<style>
  .top-progress {
    position: fixed;
    inset: 0 0 auto 0;
    height: 2px;
    z-index: 20;
    overflow: hidden;
    opacity: 0;
    transition: opacity 160ms ease;
    pointer-events: none;
  }
  .active {
    opacity: 1;
  }
  .bar {
    width: 40%;
    height: 100%;
    border-radius: 999px;
    background: linear-gradient(90deg, transparent, var(--accent), transparent);
    transform: translateX(-100%);
  }
  .active .bar {
    animation: slide 1.1s ease-in-out infinite;
  }
  @keyframes slide {
    0% {
      transform: translateX(-100%);
    }
    100% {
      transform: translateX(350%);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .active .bar {
      animation: none;
      width: 100%;
      transform: none;
      opacity: 0.6;
    }
  }
</style>
