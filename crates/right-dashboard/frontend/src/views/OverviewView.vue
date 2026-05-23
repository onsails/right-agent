<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import CostLearningRiver from '../components/charts/CostLearningRiver.vue'
import SignalTimeline from '../components/charts/SignalTimeline.vue'
import MetricCard from '../components/MetricCard.vue'
import StatusPill from '../components/StatusPill.vue'
import { money, shortDate } from '../format'
import type { DashboardOverviewResponse, DashboardSignal, LearningMarker, OverviewResponse } from '../types'

type SelectedKind = 'signal' | 'marker'

const props = defineProps<{
  overview: DashboardOverviewResponse | null
  activity: OverviewResponse | null
}>()

const selectedKind = ref<SelectedKind | null>(null)
const selectedId = ref<string | null>(null)

const selectedSignal = computed(() => {
  if (selectedKind.value !== 'signal' || selectedId.value === null) {
    return null
  }
  return props.overview?.signals.find((signal) => signal.id === selectedId.value) ?? null
})

const selectedMarker = computed(() => {
  if (selectedKind.value !== 'marker' || selectedId.value === null) {
    return null
  }
  return props.overview?.cost_learning_river.markers.find((marker) => marker.id === selectedId.value) ?? null
})

const selectedEyebrow = computed(() => {
  if (selectedMarker.value) {
    return 'Selected marker'
  }
  if (selectedSignal.value) {
    return 'Selected signal'
  }
  return 'Selected item'
})

watch([selectedSignal, selectedMarker], ([signal, marker]) => {
  if (selectedKind.value !== null && signal === null && marker === null) {
    selectedKind.value = null
    selectedId.value = null
  }
})

function selectSignal(signal: DashboardSignal): void {
  selectedKind.value = 'signal'
  selectedId.value = signal.id
}

function selectMarker(marker: LearningMarker): void {
  selectedKind.value = 'marker'
  selectedId.value = marker.id
}
</script>

<template>
  <section class="metric-grid">
    <MetricCard label="Active" :value="overview?.active_runs ?? 0" tone="active" />
    <MetricCard label="Failures" :value="overview?.recent_failures ?? 0" :tone="(overview?.recent_failures ?? 0) > 0 ? 'bad' : 'ok'" />
    <MetricCard label="Today" :value="money(overview?.today_cost_usd)" />
    <MetricCard label="Candidates" :value="overview?.learning_candidates_24h ?? 0" />
    <MetricCard label="Jobs" :value="activity?.summary.cron_count ?? 0" />
    <MetricCard label="Running cron" :value="activity?.summary.active_cron_count ?? 0" tone="active" />
  </section>

  <section v-if="overview?.warnings.length" class="notice">
    <strong>Partial data</strong>
    <span v-for="warning in overview.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
      {{ warning.message }}
    </span>
  </section>

  <section class="two-column wide-main">
    <SignalTimeline
      :signals="overview?.signals ?? []"
      :selected-id="selectedSignal?.id ?? null"
      @select="selectSignal"
    />

    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">{{ selectedEyebrow }}</p>
          <h2>{{ selectedSignal?.title ?? selectedMarker?.label ?? 'None selected' }}</h2>
        </div>
        <StatusPill v-if="selectedSignal" :status="selectedSignal.severity" />
        <StatusPill v-else-if="selectedMarker" :status="selectedMarker.severity" />
      </header>

      <p v-if="!selectedSignal && !selectedMarker" class="muted-line">Select a signal or marker</p>
      <dl v-else class="meta-grid compact">
        <div>
          <dt>When</dt>
          <dd>{{ shortDate(selectedSignal?.occurred_at ?? selectedMarker?.occurred_at) }}</dd>
        </div>
        <div>
          <dt>Source</dt>
          <dd>{{ selectedSignal?.source ?? selectedMarker?.source ?? 'none' }}</dd>
        </div>
        <div>
          <dt>Skill</dt>
          <dd>{{ selectedSignal?.related_skill_name ?? selectedMarker?.skill_name ?? 'none' }}</dd>
        </div>
        <div>
          <dt>Cost</dt>
          <dd>{{ money(selectedSignal?.cost_usd ?? selectedMarker?.cost_usd) }}</dd>
        </div>
        <div>
          <dt>Kind</dt>
          <dd>{{ selectedSignal?.kind ?? selectedMarker?.kind }}</dd>
        </div>
        <div v-if="selectedSignal?.related_run_id">
          <dt>Run</dt>
          <dd>{{ selectedSignal.related_run_id }}</dd>
        </div>
        <div v-if="selectedSignal?.related_report_id">
          <dt>Report</dt>
          <dd>{{ selectedSignal.related_report_id }}</dd>
        </div>
      </dl>
      <section v-if="selectedSignal?.detail" class="text-block">
        <h3>Detail</h3>
        <p>{{ selectedSignal.detail }}</p>
      </section>
    </aside>
  </section>

  <CostLearningRiver
    :river="overview?.cost_learning_river ?? null"
    @select-marker="selectMarker"
  />
</template>
