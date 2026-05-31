<script setup lang="ts">
import { computed } from 'vue'

import { shortDate } from '../format'
import type { LearningEventSummary } from '../types'
import StatusPill from './StatusPill.vue'
import { failureSampleLabel } from './failureSampleLabel'

const props = defineProps<{
  events: LearningEventSummary[]
  total: number
}>()

const sampleLabel = computed(() => failureSampleLabel(props.total, props.events.length))
</script>

<template>
  <p v-if="events.length === 0" class="muted-line">No failures</p>
  <p v-if="sampleLabel" class="muted-line">{{ sampleLabel }}</p>
  <div v-if="events.length > 0" class="row-list">
    <div
      v-for="event in events"
      :key="`${event.skill_name}:${event.created_at}`"
      class="data-row static"
    >
      <span class="row-main">
        <strong>{{ event.skill_name }}</strong>
        <small>{{ event.action }} / {{ shortDate(event.created_at) }}</small>
        <small v-if="event.message" class="run-note-preview">{{ event.message }}</small>
      </span>
      <span class="row-side">
        <StatusPill :status="event.status" />
      </span>
    </div>
  </div>
</template>
