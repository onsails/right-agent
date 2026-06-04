<script setup lang="ts">
import { computed, ref } from 'vue'

import { shortDate } from '../format'
import type { LearningEventSummary } from '../types'
import StatusPill from './StatusPill.vue'
import { failureSampleLabel } from './failureSampleLabel'

const props = defineProps<{
  events: LearningEventSummary[]
  total: number
}>()

const sampleLabel = computed(() => failureSampleLabel(props.total, props.events.length))
const selectedKey = ref<number | null>(null)

function toggle(key: number): void {
  selectedKey.value = selectedKey.value === key ? null : key
}
</script>

<template>
  <aside class="panel detail-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Failed skills</p>
        <h2>Recent failures</h2>
      </div>
    </header>
    <p class="muted-line explainer">
      A failed skill is a learning attempt that errored out. It is not a refusal
      - a refusal means the skill already covered the request.
    </p>
    <p v-if="events.length === 0" class="muted-line">No failures</p>
    <p v-if="sampleLabel" class="muted-line">{{ sampleLabel }}</p>
    <div v-if="events.length > 0" class="row-list">
      <template v-for="event in events" :key="event.id">
        <button
          type="button"
          class="data-row"
          :class="{ selected: selectedKey === event.id }"
          :aria-expanded="selectedKey === event.id"
          @click="toggle(event.id)"
        >
          <span class="row-main">
            <strong>{{ event.skill_name }}</strong>
            <small>{{ event.action }} / {{ shortDate(event.created_at) }}</small>
            <small v-if="event.message" class="run-note-preview">{{ event.message }}</small>
          </span>
          <span class="row-side">
            <StatusPill :status="event.status" />
          </span>
        </button>
        <dl v-if="selectedKey === event.id" class="meta-grid compact signal-detail">
          <div><dt>When</dt><dd>{{ shortDate(event.created_at) }}</dd></div>
          <div><dt>Action</dt><dd>{{ event.action }}</dd></div>
          <div><dt>Status</dt><dd>{{ event.status }}</dd></div>
          <div v-if="event.message" class="signal-detail-text"><dt>Message</dt><dd>{{ event.message }}</dd></div>
          <div v-if="event.summary" class="signal-detail-text"><dt>Summary</dt><dd>{{ event.summary }}</dd></div>
        </dl>
      </template>
    </div>
  </aside>
</template>

<style scoped>
.explainer {
  margin-bottom: 8px;
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
