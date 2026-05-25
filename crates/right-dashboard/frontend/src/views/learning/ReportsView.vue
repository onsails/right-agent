<script setup lang="ts">
import LearningFlowChart from '../../components/charts/LearningFlowChart.vue'
import LearningSignalPanel from '../../components/charts/LearningSignalPanel.vue'
import MetricCard from '../../components/MetricCard.vue'
import type { LearningOverviewResponse } from '../../types'

defineProps<{
  learning: LearningOverviewResponse | null
}>()
</script>

<template>
  <section v-if="learning?.warnings.length" class="notice">
    <strong>Partial data</strong>
    <span v-for="warning in learning.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
      {{ warning.message }}
    </span>
  </section>

  <section class="two-column wide-main">
    <LearningFlowChart
      :nodes="learning?.flow_nodes ?? []"
      :edges="learning?.flow_edges ?? []"
    />
    <LearningSignalPanel :signals="learning?.recent_learning_signals ?? []" />
  </section>

  <section class="metric-grid">
    <MetricCard label="Created 7d" :value="learning?.lifecycle.created_7d ?? 0" tone="ok" />
    <MetricCard label="Updated 7d" :value="learning?.lifecycle.updated_7d ?? 0" tone="active" />
    <MetricCard label="Failed 7d" :value="learning?.lifecycle.failed_or_aborted_7d ?? 0" tone="bad" />
  </section>
</template>
