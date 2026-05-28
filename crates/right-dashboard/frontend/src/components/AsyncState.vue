<script setup lang="ts">
import { computed } from 'vue'
import Spinner from './Spinner.vue'
import { resolveAsyncState } from './asyncState'

const props = withDefaults(defineProps<{
  loading: boolean
  error: string | null
  empty: boolean
  emptyText?: string
}>(), { emptyText: 'No data' })

const kind = computed(() => resolveAsyncState({
  loading: props.loading,
  error: props.error,
  empty: props.empty,
}))
</script>

<template>
  <p v-if="kind === 'error'" class="notice inline">{{ error }}</p>
  <div v-else-if="kind === 'loading'" class="async-loading">
    <Spinner />
  </div>
  <p v-else-if="kind === 'empty'" class="muted-line">{{ emptyText }}</p>
  <slot v-else />
</template>

<style scoped>
.async-loading {
  display: flex;
  justify-content: center;
  padding: 24px 0;
}
</style>
