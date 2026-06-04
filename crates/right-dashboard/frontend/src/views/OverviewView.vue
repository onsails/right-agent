<script setup lang="ts">
import { computed, ref } from 'vue'
import CostLearningRiver from '../components/charts/CostLearningRiver.vue'
import SignalTimeline from '../components/charts/SignalTimeline.vue'
import AsyncState from '../components/AsyncState.vue'
import CollapsibleSection from '../components/CollapsibleSection.vue'
import MetricCard from '../components/MetricCard.vue'
import RunFailureList from '../components/RunFailureList.vue'
import { failureMetric } from '../components/failureMetric'
import { money } from '../format'
import type { DashboardOverviewResponse, DashboardSignal, OverviewResponse } from '../types'

const props = defineProps<{
  overview: DashboardOverviewResponse | null
  activity: OverviewResponse | null
  loading: boolean
  error: string | null
}>()

const selectedId = ref<string | null>(null)
const failuresOpen = ref(false)
const failures = computed(() => failureMetric(props.overview?.recent_failures ?? 0))

function selectSignal(signal: DashboardSignal): void {
  selectedId.value = selectedId.value === signal.id ? null : signal.id
}
</script>

<template>
  <AsyncState :loading="loading" :error="error" :empty="overview === null && !loading" empty-text="No overview data">
    <CostLearningRiver :river="overview?.cost_learning_river ?? null" />

    <section class="metric-grid">
      <MetricCard label="Active" :value="overview?.active_runs ?? 0" tone="active" />
      <MetricCard
        label="Failures"
        :value="overview?.recent_failures ?? 0"
        :tone="failures.tone"
        :interactive="failures.interactive"
        @select="failuresOpen = !failuresOpen"
      />
      <MetricCard label="Today" :value="money(overview?.today_cost_usd)" />
      <MetricCard label="Candidates" :value="overview?.learning_candidates_24h ?? 0" />
      <MetricCard label="Jobs" :value="activity?.summary.cron_count ?? 0" />
      <MetricCard label="Running cron" :value="activity?.summary.active_cron_count ?? 0" tone="active" />
    </section>

    <CollapsibleSection
      v-if="failures.interactive"
      v-model:open="failuresOpen"
      title="Failures"
      :count="overview?.recent_failures ?? 0"
    >
      <RunFailureList :runs="overview?.recent_failed_runs ?? []" :total="overview?.recent_failures ?? 0" />
    </CollapsibleSection>

    <section v-if="overview?.warnings.length" class="notice">
      <strong>Partial data</strong>
      <span v-for="warning in overview.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
        {{ warning.message }}
      </span>
    </section>

    <SignalTimeline
      :signals="overview?.signals ?? []"
      :selected-id="selectedId"
      @select="selectSignal"
    />
  </AsyncState>
</template>
