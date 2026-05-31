<script setup lang="ts">
import { learningOverview } from '../../api'
import { useLiveResource } from '../../composables/useLiveResource'
import ReportsView from './ReportsView.vue'

const { data } = useLiveResource(learningOverview, { key: 'learning' })
</script>

<template>
  <ReportsView :learning="data" />
</template>
