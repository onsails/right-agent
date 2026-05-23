<script setup lang="ts">
import StatusPill from '../StatusPill.vue'
import { money, shortDate } from '../../format'
import type { DashboardSignal } from '../../types'

defineProps<{
  signals: DashboardSignal[]
  selectedId: string | null
}>()

const emit = defineEmits<{
  select: [signal: DashboardSignal]
}>()
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Signals</p>
        <h2>Recent changes</h2>
      </div>
    </header>

    <div v-if="signals.length === 0" class="chart-empty">No recent signals</div>
    <button
      v-for="signal in signals"
      v-else
      :key="signal.id"
      type="button"
      class="data-row tall"
      :class="{ selected: selectedId === signal.id }"
      @click="emit('select', signal)"
    >
      <span class="row-main">
        <strong>{{ signal.title }}</strong>
        <span>{{ signal.detail ?? signal.source ?? signal.kind }}</span>
        <small>{{ shortDate(signal.occurred_at) }}</small>
      </span>
      <span class="row-side">
        <StatusPill :status="signal.severity" />
        <small v-if="signal.cost_usd !== null">{{ money(signal.cost_usd) }}</small>
      </span>
    </button>
  </section>
</template>
