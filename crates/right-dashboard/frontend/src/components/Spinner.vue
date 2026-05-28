<template>
  <span class="spinner" role="status" aria-label="Loading" />
</template>

<style scoped>
.spinner {
  display: inline-block;
  width: 18px;
  height: 18px;
  border: 2px solid var(--tg-theme-hint_color, #888);
  border-top-color: transparent;
  border-radius: 50%;
  animation: spinner-rotate 0.7s linear infinite;
}
@keyframes spinner-rotate {
  to { transform: rotate(360deg); }
}
@media (prefers-reduced-motion: reduce) {
  .spinner {
    animation: none;
    border-top-color: var(--tg-theme-hint_color, #888);
    opacity: 0.5;
  }
}
</style>
