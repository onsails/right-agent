<script setup lang="ts">
defineProps<{
  label: string
  value: string | number
  tone?: 'default' | 'ok' | 'active' | 'bad'
}>()
</script>

<template>
  <article class="metric-card" :class="tone ?? 'default'">
    <span>{{ label }}</span>
    <strong>{{ value }}</strong>
  </article>
</template>
