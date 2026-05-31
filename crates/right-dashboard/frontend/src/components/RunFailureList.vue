<script setup lang="ts">
import { computed, ref } from 'vue'

import { runDetail } from '../api'
import { money, shortDate, shortId, statusTone } from '../format'
import type { RunDetailResponse, RunSummary } from '../types'
import AsyncState from './AsyncState.vue'
import { failureSampleLabel } from './failureSampleLabel'

const props = defineProps<{
  runs: RunSummary[]
  total: number
}>()

const sampleLabel = computed(() => failureSampleLabel(props.total, props.runs.length))

const selectedId = ref<string | null>(null)
const detail = ref<RunDetailResponse | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)

async function select(run: RunSummary): Promise<void> {
  if (selectedId.value === run.id) {
    selectedId.value = null
    detail.value = null
    loading.value = false
    return
  }
  selectedId.value = run.id
  detail.value = null
  error.value = null
  loading.value = true
  try {
    const result = await runDetail(run.id)
    if (selectedId.value === run.id) {
      detail.value = result
    }
  } catch (err) {
    if (selectedId.value === run.id) {
      error.value = err instanceof Error ? err.message : 'Failed to load run detail'
    }
  } finally {
    if (selectedId.value === run.id) {
      loading.value = false
    }
  }
}
</script>

<template>
  <p v-if="runs.length === 0" class="muted-line">No failures</p>
  <p v-if="sampleLabel" class="muted-line">{{ sampleLabel }}</p>
  <div v-if="runs.length > 0" class="row-list">
    <template v-for="run in runs" :key="run.id">
      <button
        class="data-row"
        :class="{ selected: selectedId === run.id }"
        type="button"
        @click="select(run)"
      >
        <span class="row-main">
          <span class="status-dot" :class="statusTone(run.status)"></span>
          <strong>{{ run.kind }}</strong>
          <small>{{ shortId(run.id) }}</small>
          <small v-if="run.producer_ref">{{ run.producer_ref }}</small>
        </span>
        <span class="row-side">
          <strong>{{ money(run.cost_usd) }}</strong>
          <small>{{ shortDate(run.finished_at ?? run.started_at) }}</small>
        </span>
      </button>

      <section v-if="selectedId === run.id" class="run-inline-detail">
        <AsyncState
          :loading="loading"
          :error="error"
          :empty="!detail || detail.run.id !== run.id"
          empty-text="No run detail"
        >
          <dl class="meta-grid compact">
            <div>
              <dt>Exit</dt>
              <dd>{{ detail!.run.exit_code ?? 'none' }}</dd>
            </div>
            <div>
              <dt>Cost</dt>
              <dd>{{ money(detail!.run.cost_usd) }}</dd>
            </div>
            <div>
              <dt>Started</dt>
              <dd>{{ shortDate(detail!.run.started_at) }}</dd>
            </div>
            <div>
              <dt>Finished</dt>
              <dd>{{ shortDate(detail!.run.finished_at) }}</dd>
            </div>
          </dl>
          <section v-if="detail!.error_message" class="text-block">
            <h3>Error</h3>
            <p>{{ detail!.error_message }}</p>
          </section>
          <section class="text-block">
            <h3>Log</h3>
            <p v-if="!detail!.log.available" class="muted-line">Log unavailable</p>
            <pre v-else>{{ detail!.log.lines.join('\n') }}<template v-if="detail!.log.truncated">
... truncated
</template></pre>
          </section>
        </AsyncState>
      </section>
    </template>
  </div>
</template>
