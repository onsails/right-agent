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
  display: flex;
  gap: 12px;
  flex-wrap: wrap;
  font-size: 0.72rem;
  color: var(--tg-theme-hint-color, #6b7b88);
  margin: 0 0 4px;
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
