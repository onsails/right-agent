<script setup lang="ts">
import { ref } from 'vue'

import StatusPill from '../StatusPill.vue'
import { shortDate } from '../../format'
import { learningSignalLabel } from './learningSignalLabel'
import type { LearningSignalPoint } from '../../types'

defineProps<{
  signals: LearningSignalPoint[]
}>()

const selectedId = ref<string | null>(null)

function toggle(id: string): void {
  selectedId.value = selectedId.value === id ? null : id
}
</script>

<template>
  <aside class="panel detail-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Learning signals</p>
        <h2>Recent outcomes</h2>
      </div>
    </header>
    <p v-if="signals.length === 0" class="muted-line">No recent learning outcomes</p>
    <div v-else class="row-list">
      <template v-for="signal in signals" :key="signal.id">
        <button
          type="button"
          class="data-row"
          :class="{ selected: selectedId === signal.id }"
          :aria-expanded="selectedId === signal.id"
          @click="toggle(signal.id)"
        >
          <span class="row-main">
            <strong>{{ signal.label }}</strong>
            <span v-if="signal.detail" class="signal-preview">{{ signal.detail }}</span>
            <small>
              {{ shortDate(signal.occurred_at) }}
              <template v-if="signal.skill_name"> / {{ signal.skill_name }}</template>
              <template v-if="signal.count > 1"> / {{ signal.count }}</template>
            </small>
          </span>
          <span class="row-side">
            <StatusPill :status="signal.severity" :label="learningSignalLabel(signal.kind)" />
          </span>
        </button>
        <dl v-if="selectedId === signal.id" class="meta-grid compact signal-detail">
          <div><dt>When</dt><dd>{{ shortDate(signal.occurred_at) }}</dd></div>
          <div><dt>Kind</dt><dd>{{ signal.kind }}</dd></div>
          <div v-if="signal.skill_name"><dt>Skill</dt><dd>{{ signal.skill_name }}</dd></div>
          <div v-if="signal.detail" class="signal-detail-text"><dt>Detail</dt><dd>{{ signal.detail }}</dd></div>
        </dl>
      </template>
    </div>
  </aside>
</template>

<style scoped>
.signal-preview {
  display: block;
  opacity: 0.85;
}
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
