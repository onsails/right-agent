<script setup lang="ts">
import { computed } from 'vue'
import AsyncVChart from './AsyncVChart.vue'
import { money } from '../../format'
import type { CostLearningRiver, LearningMarker } from '../../types'

type ThemeRiverDatum = [bucket: string, costUsd: number, source: string]

interface MarkerDatum {
  name: string
  value: [occurredAt: string, lane: number]
  markerId: string
  marker: LearningMarker
}

interface TooltipDatum {
  data?: unknown
  seriesName?: string
}

interface ChartClickEvent {
  data?: unknown
  seriesType?: unknown
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

function isLearningMarker(value: unknown): value is LearningMarker {
  return typeof value === 'object' &&
    value !== null &&
    typeof (value as LearningMarker).id === 'string' &&
    typeof (value as LearningMarker).occurred_at === 'string' &&
    typeof (value as LearningMarker).label === 'string'
}

function isMarkerDatum(value: unknown): value is MarkerDatum {
  return typeof value === 'object' &&
    value !== null &&
    typeof (value as MarkerDatum).markerId === 'string' &&
    Array.isArray((value as MarkerDatum).value) &&
    typeof (value as MarkerDatum).value[0] === 'string' &&
    isLearningMarker((value as MarkerDatum).marker)
}

function formatTooltip(params: unknown): string {
  const rows = Array.isArray(params) ? params : [params]
  return rows.map((row) => {
    if (!isTooltipDatum(row)) {
      return ''
    }
    if (isMarkerDatum(row.data)) {
      const cost = row.data.marker.cost_usd === null ? '' : ` (${money(row.data.marker.cost_usd)})`
      return `${row.data.marker.label}${cost}`
    }
    if (isThemeRiverDatum(row.data)) {
      return `${row.data[2] || row.seriesName || 'source'}: ${money(row.data[1])}`
    }
    return ''
  }).filter(Boolean).join('\n')
}

function selectMarkerFromChart(event: ChartClickEvent): void {
  if (event.seriesType !== 'scatter' || !isMarkerDatum(event.data)) {
    return
  }
  emit('selectMarker', event.data.marker)
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

  const markers: MarkerDatum[] = (river.markers ?? []).map((marker, index) => ({
    name: marker.label,
    value: [marker.occurred_at, index % 2],
    markerId: marker.id,
    marker,
  }))

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
      ...(markers.length === 0 ? [] : [{
        name: 'Learning markers',
        type: 'scatter',
        coordinateSystem: 'singleAxis',
        symbol: 'pin',
        symbolSize: 24,
        z: 10,
        label: {
          show: true,
          formatter: ({ data: datum }: { data: unknown }) => isMarkerDatum(datum) ? datum.marker.label : '',
          position: 'top',
          distance: 6,
          overflow: 'truncate',
          width: 96,
        },
        emphasis: {
          scale: 1.2,
          label: { width: 140 },
        },
        itemStyle: {
          color: '#f59e0b',
          borderColor: '#111827',
          borderWidth: 1,
        },
        data: markers,
      }]),
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
      @click="selectMarkerFromChart"
    />

    <div v-if="(river?.markers ?? []).length" class="marker-list">
      <button
        v-for="marker in (river?.markers ?? [])"
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
