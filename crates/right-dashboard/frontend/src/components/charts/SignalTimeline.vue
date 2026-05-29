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
    <template v-for="signal in signals" v-else :key="signal.id">
      <button
        type="button"
        class="data-row tall"
        :class="{ selected: selectedId === signal.id }"
        :aria-expanded="selectedId === signal.id"
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
      <dl v-if="selectedId === signal.id" class="meta-grid compact signal-detail">
        <div><dt>When</dt><dd>{{ shortDate(signal.occurred_at) }}</dd></div>
        <div><dt>Source</dt><dd>{{ signal.source ?? 'none' }}</dd></div>
        <div><dt>Skill</dt><dd>{{ signal.related_skill_name ?? 'none' }}</dd></div>
        <div><dt>Cost</dt><dd>{{ signal.cost_usd !== null ? money(signal.cost_usd) : 'none' }}</dd></div>
        <div><dt>Kind</dt><dd>{{ signal.kind }}</dd></div>
        <div v-if="signal.related_run_id"><dt>Run</dt><dd>{{ signal.related_run_id }}</dd></div>
        <div v-if="signal.related_report_id"><dt>Report</dt><dd>{{ signal.related_report_id }}</dd></div>
        <div v-if="signal.detail" class="signal-detail-text"><dt>Detail</dt><dd>{{ signal.detail }}</dd></div>
      </dl>
    </template>
  </section>
</template>

<style scoped>
.signal-detail {
  padding: 8px 12px 14px;
  background: var(--tg-theme-secondary-bg-color, rgba(127, 127, 127, 0.1));
  border-radius: 0 0 10px 10px;
  margin-bottom: 8px;
}
.signal-detail-text {
  grid-column: 1 / -1;
}
</style>
