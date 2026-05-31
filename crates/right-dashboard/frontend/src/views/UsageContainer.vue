<script setup lang="ts">
import { usageOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import UsageView from './UsageView.vue'

const { data, loading, error } = useLiveResource(usageOverview, { key: 'usage' })
</script>

<template>
  <UsageView :usage="data" :loading="loading" :error="error" />
</template>
