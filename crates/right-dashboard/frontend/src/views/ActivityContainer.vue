<script setup lang="ts">
import { ref, watch } from 'vue'

import { DashboardApiError, overview as activityOverview, runDetail } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import { activityContainsRun } from './activitySelection'
import ActivityView from './ActivityView.vue'
import type { RunDetailResponse, RunSummary } from '../types'

const { data: activity, refresh } = useLiveResource(activityOverview, { key: 'activity' })

const selectedRun = ref<RunDetailResponse | null>(null)
const selectedRunId = ref<string | null>(null)
const loadingDetail = ref(false)
const detailError = ref<string | null>(null)

async function selectRun(run: RunSummary): Promise<void> {
  const runId = run.id
  selectedRunId.value = runId
  selectedRun.value = null
  loadingDetail.value = true
  detailError.value = null
  try {
    const detail = await runDetail(runId)
    if (selectedRunId.value === runId) {
      selectedRun.value = detail
    }
  } catch (err) {
    if (err instanceof DashboardApiError && err.isLocked) {
      void refresh()
    }
    if (selectedRunId.value === runId) {
      detailError.value = err instanceof Error ? err.message : 'Run unavailable'
    }
  } finally {
    if (selectedRunId.value === runId) {
      loadingDetail.value = false
    }
  }
}

watch(activity, (value) => {
  if (selectedRunId.value !== null && !activityContainsRun(value, selectedRunId.value)) {
    selectedRunId.value = null
    selectedRun.value = null
    detailError.value = null
  }
})
</script>

<template>
  <ActivityView
    :overview="activity"
    :selected-run="selectedRun"
    :selected-run-id="selectedRunId"
    :loading-detail="loadingDetail"
    :detail-error="detailError"
    @select-run="selectRun"
  />
</template>
