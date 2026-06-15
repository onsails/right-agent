<script setup lang="ts">
import { computed } from 'vue'
import AsyncState from '../../components/AsyncState.vue'
import CollapsibleSection from '../../components/CollapsibleSection.vue'
import type { CuratorConsolidation, CuratorRunSummary } from '../../types'
import { curatorRunHeadline, curatorRunStatusTone } from './curatorRuns'

const props = defineProps<{
  runs: CuratorRunSummary[] | null
  consolidations: CuratorConsolidation[] | null
}>()

const runs = computed(() => props.runs ?? [])
const consolidations = computed(() => props.consolidations ?? [])
const loading = computed(() => props.runs === null)
</script>

<template>
  <section class="panel">
    <h3>Curator</h3>
    <AsyncState :loading="loading" :error="null" :empty="runs.length === 0" emptyText="No curator runs yet">
      <CollapsibleSection title="Recent runs" :count="runs.length" :defaultOpen="true">
        <ul class="curator-runs">
          <li v-for="run in runs" :key="run.run_at" :class="curatorRunStatusTone(run.status)">
            <span class="when">{{ run.run_at }}</span>
            <span class="trigger">{{ run.trigger }}</span>
            <span class="mode">{{ run.mode }}</span>
            <span class="headline">{{ curatorRunHeadline(run) }}</span>
            <span class="cost">${{ run.cost_usd.toFixed(3) }}</span>
          </li>
        </ul>
      </CollapsibleSection>
      <CollapsibleSection title="Consolidations" :count="consolidations.length" :defaultOpen="true">
        <ul class="curator-lineage">
          <li v-for="c in consolidations" :key="c.absorbed">{{ c.absorbed }} → {{ c.umbrella }}</li>
        </ul>
      </CollapsibleSection>
    </AsyncState>
  </section>
</template>
