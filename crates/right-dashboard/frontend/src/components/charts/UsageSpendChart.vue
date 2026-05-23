<script setup lang="ts">
import { computed } from 'vue'
import AsyncVChart from './AsyncVChart.vue'
import { money } from '../../format'
import type { UsageDailyPoint } from '../../types'

interface TooltipRow {
  seriesName?: string
  value?: unknown
}

interface ChartClickEvent {
  name?: unknown
}

type BarDatum = number | {
  value: number
  itemStyle: {
    borderColor: string
    borderWidth: number
  }
}

const props = defineProps<{
  points: UsageDailyPoint[]
  selectedDate: string | null
}>()

const emit = defineEmits<{
  selectDate: [date: string]
}>()

const sources = computed(() =>
  Array.from(new Set(props.points.flatMap((point) => (point.sources ?? []).map((source) => source.source)))),
)

const hasSpendData = computed(() =>
  props.points.some((point) => (point.sources ?? []).some((source) => source.cost_usd > 0)),
)

function isTooltipRow(value: unknown): value is TooltipRow {
  return typeof value === 'object' && value !== null
}

function numericValue(value: unknown): number {
  return typeof value === 'number' ? value : 0
}

function formatTooltip(params: unknown): string {
  const rows = Array.isArray(params) ? params : [params]
  return rows.map((row) => {
    if (!isTooltipRow(row)) {
      return ''
    }
    return `${row.seriesName ?? 'source'}: ${money(numericValue(row.value))}`
  }).filter(Boolean).join('<br>')
}

function selectDate(event: ChartClickEvent): void {
  if (typeof event.name === 'string') {
    emit('selectDate', event.name)
  }
}

function barDatum(point: UsageDailyPoint, source: string): BarDatum {
  const value = (point.sources ?? []).find((row) => row.source === source)?.cost_usd ?? 0
  if (point.date !== props.selectedDate) {
    return value
  }
  return {
    value,
    itemStyle: {
      borderColor: '#111827',
      borderWidth: 1,
    },
  }
}

const option = computed(() => ({
  tooltip: {
    trigger: 'axis',
    axisPointer: { type: 'shadow' },
    formatter: formatTooltip,
  },
  legend: { type: 'scroll', bottom: 0 },
  grid: { left: 44, right: 12, top: 18, bottom: 54 },
  xAxis: { type: 'category', data: props.points.map((point) => point.date), axisLabel: { hideOverlap: true } },
  yAxis: { type: 'value' },
  series: sources.value.map((source) => ({
    name: source,
    type: 'bar',
    stack: 'cost',
    emphasis: { focus: 'series' },
    data: props.points.map((point) => barDatum(point, source)),
  })),
}))
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Spend</p>
        <h2>Last 30 days</h2>
      </div>
    </header>
    <div v-if="!hasSpendData" class="chart-empty">No usage data</div>
    <AsyncVChart
      v-else
      class="dashboard-chart"
      :option="option"
      autoresize
      @click="selectDate"
    />
  </section>
</template>
