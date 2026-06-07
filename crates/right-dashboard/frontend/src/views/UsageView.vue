<script setup lang="ts">
import { computed, ref, watchEffect } from 'vue'
import UsageBreakdown from '../components/charts/UsageBreakdown.vue'
import UsageSpendChart from '../components/charts/UsageSpendChart.vue'
import AsyncState from '../components/AsyncState.vue'
import TokenLine from '../components/charts/TokenLine.vue'
import TokenLegend from '../components/charts/TokenLegend.vue'
import { money } from '../format'
import type { UsageCronJobSummary, UsageOverviewResponse, UsageRange, UsageWindow } from '../types'
import { selectedDayRangeLabel } from './usageDayRange'
import { USAGE_RANGE_OPTIONS } from './usageRanges'

const props = defineProps<{
  usage: UsageOverviewResponse | null
  loading: boolean
  error: string | null
  selectedRange: UsageRange
}>()

const emit = defineEmits<{
  selectRange: [range: UsageRange]
}>()

const selectedDate = ref<string | null>(null)

watchEffect(() => {
  const points = props.usage?.daily_series ?? []
  if (points.length === 0) {
    selectedDate.value = null
    return
  }

  if (selectedDate.value === null || !points.some((point) => point.date === selectedDate.value)) {
    selectedDate.value = points[points.length - 1].date
  }
})

const selectedPoint = computed(() =>
  props.usage?.daily_series.find((point) => point.date === selectedDate.value) ?? null,
)

const selectedRangeLabel = computed(() =>
  selectedDayRangeLabel(selectedDate.value, props.usage?.timezone ?? 'UTC', props.usage?.generated_at ?? new Date().toISOString()),
)

const selectedWindow = computed(() => props.usage?.window ?? null)
const cronJobs = computed(() => props.usage?.cron_jobs ?? [])
const cronTotalCost = computed(() => cronJobs.value.reduce((sum, job) => sum + job.cost_usd, 0))

function windowRows(window: UsageWindow | null | undefined) {
  return window?.sources ?? []
}

function cronJobModels(job: UsageCronJobSummary) {
  return job.per_model ?? []
}
</script>

<template>
  <nav class="segmented" aria-label="Usage range">
    <button
      v-for="option in USAGE_RANGE_OPTIONS"
      :key="option.key"
      type="button"
      class="segment-button"
      :class="{ active: selectedRange === option.key }"
      :aria-pressed="selectedRange === option.key"
      @click="emit('selectRange', option.key)"
    >
      {{ option.label }}
    </button>
  </nav>

  <AsyncState :loading="loading" :error="error" :empty="usage === null && !loading" empty-text="No usage data">
    <section v-if="usage?.warnings.length" class="notice">
      <strong>Partial data</strong>
      <span v-for="warning in usage.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
        {{ warning.message }}
      </span>
    </section>

    <TokenLegend :sticky="false" />

    <section class="two-column wide-main">
      <UsageSpendChart
        :points="usage?.daily_series ?? []"
        :selected-date="selectedDate"
        @select-date="selectedDate = $event"
      />
      <UsageBreakdown :point="selectedPoint" :range-label="selectedRangeLabel" />
    </section>

    <section class="list-stack">
      <article v-if="selectedWindow" class="panel">
        <header class="panel-head">
          <div>
            <p class="eyebrow">{{ selectedWindow.key }}</p>
            <h2>{{ selectedWindow.label }}</h2>
            <p class="muted-line">{{ selectedWindow.range_label }}</p>
          </div>
          <strong>{{ money(selectedWindow.total_cost_usd) }}</strong>
        </header>
        <p v-if="selectedWindow.budget_skip_count > 0" class="muted-line">
          Budget-blocked learning attempts: {{ selectedWindow.budget_skip_count }}
        </p>

        <div class="model-grid">
          <div v-for="source in windowRows(selectedWindow)" :key="source.source" class="usage-source">
            <div class="model-row">
              <span>{{ source.source }}</span>
              <strong>{{ money(source.cost_usd) }}</strong>
            </div>
            <TokenLine :tokens="source" compact />
          </div>
        </div>
      </article>

      <article v-if="!selectedWindow" class="empty-panel">No usage data for period</article>

      <article v-if="usage" class="panel">
        <header class="panel-head">
          <div>
            <p class="eyebrow">Cron</p>
            <h2>Cron jobs</h2>
            <p class="muted-line">{{ selectedWindow?.range_label }}</p>
          </div>
          <strong>{{ money(cronTotalCost) }}</strong>
        </header>

        <div v-if="cronJobs.length > 0" class="model-grid">
          <div v-for="job in cronJobs" :key="job.job_name" class="usage-source">
            <div class="model-row">
              <span>{{ job.job_name }}</span>
              <strong>{{ money(job.cost_usd) }}</strong>
            </div>
            <TokenLine :tokens="job" compact />
            <div v-if="cronJobModels(job).length > 0" class="row-list">
              <div v-for="model in cronJobModels(job)" :key="model.model" class="model-row">
                <span>{{ model.model }}</span>
                <strong>{{ money(model.cost_usd) }}</strong>
              </div>
            </div>
          </div>
        </div>
        <p v-else class="muted-line">No cron usage for period</p>
      </article>
    </section>

    <TokenLegend />
  </AsyncState>
</template>
