<script setup lang="ts">
import { computed } from 'vue'
import AsyncVChart from './AsyncVChart.vue'
import { jewelChartBase } from './jewelChart'
import { money } from '../../format'
import type { CostLearningRiver } from '../../types'

type ThemeRiverDatum = [bucket: string, costUsd: number, source: string]

interface TooltipDatum {
  data?: unknown
  seriesName?: string
}

const props = defineProps<{
  river: CostLearningRiver | null
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
    if (!isTooltipDatum(row)) {
      return ''
    }
    if (isThemeRiverDatum(row.data)) {
      return `${row.data[2] || row.seriesName || 'source'}: ${money(row.data[1])}`
    }
    return ''
  }).filter(Boolean).join('\n')
}

const option = computed(() => {
  const river = props.river
  if (!river || river.points.length === 0) {
    return null
  }

  const data: ThemeRiverDatum[] = river.points.flatMap((point) =>
    (point.sources ?? []).map((source): ThemeRiverDatum => [point.bucket, source.cost_usd, source.source]),
  )

  if (data.length === 0) {
    return null
  }

  return {
    ...jewelChartBase,
    tooltip: {
      ...jewelChartBase.tooltip,
      trigger: 'axis',
      renderMode: 'richText',
      formatter: formatTooltip,
    },
    legend: {
      ...jewelChartBase.legend,
      type: 'scroll',
      bottom: 0,
    },
    singleAxis: {
      type: 'time',
      top: 16,
      bottom: 52,
      axisLine: { lineStyle: { color: '#2d2533' } },
      axisLabel: { color: '#b6a8b0', hideOverlap: true },
      splitLine: { lineStyle: { color: '#2d2533' } },
    },
    dataZoom: [
      {
        type: 'inside',
        singleAxisIndex: 0,
        zoomOnMouseWheel: true,
        moveOnMouseWheel: false,
        start: 0,
        end: 100,
      },
      {
        type: 'slider',
        singleAxisIndex: 0,
        bottom: 26,
        height: 18,
        start: 0,
        end: 100,
      },
    ],
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
    <AsyncVChart
      v-else
      class="dashboard-chart"
      :option="option"
      autoresize
    />
  </section>
</template>
