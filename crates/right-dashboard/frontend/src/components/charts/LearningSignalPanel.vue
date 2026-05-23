<script setup lang="ts">
import StatusPill from '../StatusPill.vue'
import { shortDate } from '../../format'
import type { LearningSignalPoint } from '../../types'

defineProps<{
  signals: LearningSignalPoint[]
}>()
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
      <div v-for="signal in signals" :key="signal.id" class="data-row static">
        <span class="row-main">
          <strong>{{ signal.label }}</strong>
          <small>
            {{ shortDate(signal.occurred_at) }}
            <template v-if="signal.skill_name"> / {{ signal.skill_name }}</template>
            <template v-if="signal.count > 1"> / {{ signal.count }}</template>
          </small>
        </span>
        <span class="row-side">
          <StatusPill :status="signal.severity" />
        </span>
      </div>
    </div>
  </aside>
</template>
