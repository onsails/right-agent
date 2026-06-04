<script setup lang="ts">
import { computed } from 'vue'

import LearningFlowChart from '../../components/charts/LearningFlowChart.vue'
import LearningSignalPanel from '../../components/charts/LearningSignalPanel.vue'
import FailedSkillList from '../../components/FailedSkillList.vue'
import MetricCard from '../../components/MetricCard.vue'
import { failureMetric } from '../../components/failureMetric'
import type { LearningOverviewResponse } from '../../types'

const props = defineProps<{
  learning: LearningOverviewResponse | null
}>()

const failureTone = computed(() => failureMetric(props.learning?.lifecycle.failed_7d ?? 0).tone)
const refusedCount = computed(() => props.learning?.lifecycle.refused_7d ?? 0)
</script>

<template>
  <section v-if="learning?.warnings.length" class="notice">
    <strong>Partial data</strong>
    <span v-for="warning in learning.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
      {{ warning.message }}
    </span>
  </section>

  <section class="chart-panel-wrap">
    <LearningFlowChart
      :nodes="learning?.flow_nodes ?? []"
      :edges="learning?.flow_edges ?? []"
    />
  </section>

  <section class="metric-grid">
    <MetricCard label="Created 7d" :value="learning?.lifecycle.created_7d ?? 0" tone="ok" />
    <MetricCard label="Updated 7d" :value="learning?.lifecycle.updated_7d ?? 0" tone="active" />
    <MetricCard label="Failed 7d" :value="learning?.lifecycle.failed_7d ?? 0" :tone="failureTone" />
  </section>

  <section class="two-column">
    <LearningSignalPanel :signals="learning?.recent_learning_signals ?? []" />
    <FailedSkillList
      :events="learning?.lifecycle.recent_failed_events ?? []"
      :total="learning?.lifecycle.failed_7d ?? 0"
    />
  </section>

  <p v-if="refusedCount > 0" class="muted-line refusals-caption">
    Refused {{ refusedCount }} - the skill already covered the request; nothing changed.
  </p>
</template>

<style scoped>
.chart-panel-wrap {
  margin-bottom: 10px;
}

.refusals-caption {
  margin-top: 8px;
}
</style>
