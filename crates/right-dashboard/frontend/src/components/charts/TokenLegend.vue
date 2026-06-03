<script setup lang="ts"></script>

<template>
  <div class="token-legend">
    <span class="lg lg-input">input</span>
    <span class="lg lg-output">output</span>
    <span class="lg lg-create">cache create</span>
    <span class="lg lg-read">cache read</span>
  </div>
</template>

<style scoped>
.token-legend {
  position: sticky;
  bottom: 0;
  z-index: 15;
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  font-size: 0.72rem;
  color: var(--tg-theme-hint-color, #6b7b88);
  margin: 8px -12px 0;
  padding: 8px 12px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
}

@media (max-width: 560px) {
  .token-legend {
    /* clear the fixed mobile nav bar (.app-shell reserves 78px for it) */
    bottom: calc(78px + env(safe-area-inset-bottom));
  }
}
.lg {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.lg::before {
  content: '';
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--dot);
}
.lg-input { --dot: var(--token-input); }
.lg-output { --dot: var(--token-output); }
.lg-create { --dot: var(--token-create); }
.lg-read { --dot: var(--token-read); }
</style>
