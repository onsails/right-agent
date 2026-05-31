<script setup lang="ts">
defineProps<{
  label: string
  value: string | number
  tone?: 'default' | 'ok' | 'active' | 'bad'
  interactive?: boolean
}>()

defineEmits<{
  select: []
}>()
</script>

<template>
  <button
    v-if="interactive"
    type="button"
    class="metric-card metric-card-interactive"
    :class="tone ?? 'default'"
    @click="$emit('select')"
  >
    <span>{{ label }}</span>
    <strong>{{ value }}</strong>
  </button>
  <article v-else class="metric-card" :class="tone ?? 'default'">
    <span>{{ label }}</span>
    <strong>{{ value }}</strong>
  </article>
</template>

<style scoped>
.metric-card-interactive {
  display: block;
  width: 100%;
  font: inherit;
  text-align: left;
  cursor: pointer;
  color: inherit;
}
.metric-card-interactive:hover,
.metric-card-interactive:focus-visible {
  border-color: var(--tg-theme-link-color, #2f6feb);
}
.metric-card-interactive strong::after {
  content: ' ›';
  color: var(--tg-theme-hint-color, #6b7b88);
}
</style>
