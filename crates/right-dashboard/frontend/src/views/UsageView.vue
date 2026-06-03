<script setup lang="ts">
import { computed, ref, watchEffect } from 'vue'
import UsageBreakdown from '../components/charts/UsageBreakdown.vue'
import UsageSpendChart from '../components/charts/UsageSpendChart.vue'
import AsyncState from '../components/AsyncState.vue'
import TokenLine from '../components/charts/TokenLine.vue'
import TokenLegend from '../components/charts/TokenLegend.vue'
import { money } from '../format'
import type { UsageOverviewResponse, UsageWindow } from '../types'

const props = defineProps<{
  usage: UsageOverviewResponse | null
  loading: boolean
  error: string | null
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

function windowRows(window: UsageWindow | null | undefined) {
  return window?.sources ?? []
}
</script>

<template>
  <AsyncState :loading="loading" :error="error" :empty="usage === null && !loading" empty-text="No usage data">
    <section v-if="usage?.warnings.length" class="notice">
      <strong>Partial data</strong>
      <span v-for="warning in usage.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
        {{ warning.message }}
      </span>
    </section>

    <section class="two-column wide-main">
      <UsageSpendChart
        :points="usage?.daily_series ?? []"
        :selected-date="selectedDate"
        @select-date="selectedDate = $event"
      />
      <UsageBreakdown :point="selectedPoint" />
    </section>

    <section class="list-stack">
      <article v-for="window in usage?.windows ?? []" :key="window.key" class="panel">
        <header class="panel-head">
          <div>
            <p class="eyebrow">{{ window.key }}</p>
            <h2>{{ window.label }}</h2>
          </div>
          <strong>{{ money(window.total_cost_usd) }}</strong>
        </header>
        <p v-if="window.budget_skip_count > 0" class="muted-line">
          Budget-blocked learning attempts: {{ window.budget_skip_count }}
        </p>

        <div class="model-grid">
          <div v-for="source in windowRows(window)" :key="source.source" class="usage-source">
            <div class="model-row">
              <span>{{ source.source }}</span>
              <strong>{{ money(source.cost_usd) }}</strong>
            </div>
            <TokenLine :tokens="source" compact />
          </div>
        </div>
      </article>

      <article v-if="(usage?.windows ?? []).length === 0" class="empty-panel">No usage data for period</article>
    </section>

    <TokenLegend />
  </AsyncState>
</template>
