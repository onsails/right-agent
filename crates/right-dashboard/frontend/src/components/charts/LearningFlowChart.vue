<script setup lang="ts">
import { computed } from 'vue'
import type { ComposeOption, SankeySeriesOption, TooltipComponentOption } from 'echarts'
import VChart from 'vue-echarts'
import { registerDashboardCharts } from '../../charts'
import type { LearningFlowEdge, LearningFlowNode } from '../../types'

registerDashboardCharts()

type LearningFlowChartOption = ComposeOption<SankeySeriesOption | TooltipComponentOption>

interface ChartClickEvent {
  dataType?: unknown
  name?: unknown
}

const props = defineProps<{
  nodes: LearningFlowNode[]
  edges: LearningFlowEdge[]
}>()

const emit = defineEmits<{
  selectNode: [nodeId: string]
}>()

const activeEdges = computed(() => props.edges.filter((edge) => edge.count > 0))
const hasFlowData = computed(() => props.nodes.length > 0 && activeEdges.value.length > 0)

function selectNode(event: ChartClickEvent): void {
  if (event.dataType === 'node' && typeof event.name === 'string' && event.name.length > 0) {
    emit('selectNode', event.name)
  }
}

const option = computed<LearningFlowChartOption>(() => ({
  tooltip: { trigger: 'item' },
  series: [
    {
      type: 'sankey',
      nodeAlign: 'justify',
      emphasis: { focus: 'adjacency' },
      data: props.nodes.map((node) => ({
        name: node.id,
        label: { formatter: `${node.label} (${node.count})` },
        value: node.count,
      })),
      links: activeEdges.value.map((edge) => ({
        source: edge.source,
        target: edge.target,
        value: edge.count,
      })),
    },
  ],
}))
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Learning flow</p>
        <h2>Last 7 days</h2>
      </div>
    </header>
    <div v-if="!hasFlowData" class="chart-empty">No learning flow data</div>
    <VChart
      v-else
      class="dashboard-chart"
      :option="option"
      autoresize
      @click="selectNode"
    />
  </section>
</template>
