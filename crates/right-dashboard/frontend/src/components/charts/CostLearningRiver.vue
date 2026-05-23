<script setup lang="ts">
import { computed } from 'vue'
import VChart from 'vue-echarts'
import { registerDashboardCharts } from '../../charts'
import { money } from '../../format'
import type { CostLearningRiver, LearningMarker } from '../../types'

registerDashboardCharts()

type ThemeRiverDatum = [bucket: string, costUsd: number, source: string]

interface TooltipDatum {
  data?: unknown
  seriesName?: string
}

const props = defineProps<{
  river: CostLearningRiver | null
}>()

const emit = defineEmits<{
  selectMarker: [marker: LearningMarker]
}>()

function isTooltipDatum(value: unknown): value is TooltipDatum {
  return typeof value === 'object' && value !== null
}

function isThemeRiverDatum(value: unknown): value is ThemeRiverDatum {
  return Array.isArray(value) &&
    typeof value[0] === 'string' &&
    typeof value[1] === 'number' &&
    typeof value[2] === 'string'
}

function formatTooltip(params: unknown): string {
  const rows = Array.isArray(params) ? params : [params]
  return rows.map((row) => {
    if (!isTooltipDatum(row) || !isThemeRiverDatum(row.data)) {
      return ''
    }
    return `${row.data[2] || row.seriesName || 'source'}: ${money(row.data[1])}`
  }).filter(Boolean).join('\n')
}

const option = computed(() => {
  const river = props.river
  if (!river || river.points.length === 0) {
    return null
  }

  const data: ThemeRiverDatum[] = river.points.flatMap((point) =>
    point.sources.map((source): ThemeRiverDatum => [point.bucket, source.cost_usd, source.source]),
  )

  if (data.length === 0) {
    return null
  }

  return {
    tooltip: {
      trigger: 'axis',
      renderMode: 'richText',
      formatter: formatTooltip,
    },
    legend: {
      type: 'scroll',
      bottom: 0,
    },
    singleAxis: {
      type: 'time',
      top: 16,
      bottom: 42,
      axisLabel: { hideOverlap: true },
    },
    series: [
      {
        type: 'themeRiver',
        emphasis: { focus: 'series' },
        data,
      },
    ],
  }
})
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Cost and learning</p>
        <h2>{{ river?.window ?? 'last_30_days' }}</h2>
      </div>
    </header>

    <div v-if="!option" class="chart-empty">No cost data</div>
    <VChart v-else class="dashboard-chart" :option="option" autoresize />

    <div v-if="river?.markers.length" class="marker-list">
      <button
        v-for="marker in river.markers"
        :key="marker.id"
        type="button"
        class="marker-chip"
        @click="emit('selectMarker', marker)"
      >
        {{ marker.label }}
      </button>
    </div>
  </section>
</template>
