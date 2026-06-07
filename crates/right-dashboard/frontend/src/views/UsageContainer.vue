<script setup lang="ts">
import { ref } from 'vue'
import { usageOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import type { UsageRange } from '../types'
import UsageView from './UsageView.vue'
import { DEFAULT_USAGE_RANGE } from './usageRanges'

const selectedRange = ref<UsageRange>(DEFAULT_USAGE_RANGE)
const { data, loading, error, refresh } = useLiveResource(
  () => usageOverview({ range: selectedRange.value }),
  { key: 'usage' },
)

function selectRange(range: UsageRange): void {
  if (selectedRange.value === range) {
    return
  }
  selectedRange.value = range
  void refresh({ force: true, reset: true })
}
</script>

<template>
  <UsageView
    :usage="data"
    :loading="loading"
    :error="error"
    :selected-range="selectedRange"
    @select-range="selectRange"
  />
</template>
