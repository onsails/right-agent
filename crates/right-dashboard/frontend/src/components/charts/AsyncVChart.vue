<script setup lang="ts">
import { defineAsyncComponent } from 'vue'

const VChart = defineAsyncComponent(async () => {
  const [{ default: component }, charts] = await Promise.all([
    import('vue-echarts'),
    import('../../charts'),
  ])
  charts.registerDashboardCharts()
  return component
})
</script>

<template>
  <VChart v-bind="$attrs" />
</template>
