<script setup lang="ts">
import { computed, ref } from 'vue'
import CostLearningRiver from '../components/charts/CostLearningRiver.vue'
import SignalTimeline from '../components/charts/SignalTimeline.vue'
import AsyncState from '../components/AsyncState.vue'
import CollapsibleSection from '../components/CollapsibleSection.vue'
import MetricCard from '../components/MetricCard.vue'
import RunFailureList from '../components/RunFailureList.vue'
import StatusPill from '../components/StatusPill.vue'
import { failureMetric } from '../components/failureMetric'
import { money, shortDate } from '../format'
import type { DashboardOverviewResponse, DashboardSignal, LearningMarker, OverviewResponse } from '../types'

const props = defineProps<{
  overview: DashboardOverviewResponse | null
  activity: OverviewResponse | null
  loading: boolean
  error: string | null
}>()

const selectedId = ref<string | null>(null)
const selectedMarkerId = ref<string | null>(null)
const failuresOpen = ref(false)
const failures = computed(() => failureMetric(props.overview?.recent_failures ?? 0))

const selectedMarker = computed(() =>
  (props.overview?.cost_learning_river?.markers ?? []).find((m) => m.id === selectedMarkerId.value) ?? null,
)

const markerCostLabel = computed(() => {
  const cost = selectedMarker.value?.cost_usd
  return cost !== null && cost !== undefined ? money(cost) : 'none'
})

function selectSignal(signal: DashboardSignal): void {
  selectedId.value = selectedId.value === signal.id ? null : signal.id
  if (selectedId.value !== null) {
    selectedMarkerId.value = null
  }
}

function selectMarker(marker: LearningMarker): void {
  selectedMarkerId.value = selectedMarkerId.value === marker.id ? null : marker.id
  if (selectedMarkerId.value !== null) {
    selectedId.value = null
  }
}
</script>

<template>
  <AsyncState :loading="loading" :error="error" :empty="overview === null && !loading" empty-text="No overview data">
    <CostLearningRiver :river="overview?.cost_learning_river ?? null" @select-marker="selectMarker" />

    <template v-if="selectedMarker">
      <div class="marker-detail-header">
        <span class="marker-detail-label">{{ selectedMarker.label }}</span>
        <StatusPill :status="selectedMarker.severity" />
      </div>
      <dl class="meta-grid compact marker-detail">
        <div><dt>When</dt><dd>{{ shortDate(selectedMarker.occurred_at) }}</dd></div>
        <div><dt>Source</dt><dd>{{ selectedMarker.source ?? 'none' }}</dd></div>
        <div><dt>Skill</dt><dd>{{ selectedMarker.skill_name ?? 'none' }}</dd></div>
        <div><dt>Cost</dt><dd>{{ markerCostLabel }}</dd></div>
        <div><dt>Kind</dt><dd>{{ selectedMarker.kind }}</dd></div>
      </dl>
    </template>

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
      v-if="(overview?.recent_failures ?? 0) > 0"
      v-model:open="failuresOpen"
      title="Failures"
      :count="overview?.recent_failures ?? 0"
    >
      <RunFailureList :runs="overview?.recent_failed_runs ?? []" />
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

<style scoped>
.marker-detail-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px 4px;
  background: var(--tg-theme-secondary-bg-color, rgba(127, 127, 127, 0.1));
  border-radius: 10px 10px 0 0;
}
.marker-detail-label {
  font-weight: 600;
}
.marker-detail {
  padding: 4px 12px 14px;
  background: var(--tg-theme-secondary-bg-color, rgba(127, 127, 127, 0.1));
  border-radius: 0 0 10px 10px;
  margin-bottom: 8px;
}
</style>
