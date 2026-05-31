<script setup lang="ts">
import { dashboardOverview, overview as activityOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import OverviewView from './OverviewView.vue'

const { data: overviewData, loading, error } = useLiveResource(dashboardOverview, { key: 'overview' })
const { data: activityData } = useLiveResource(activityOverview, { key: 'overview-activity' })
</script>

<template>
  <OverviewView
    :overview="overviewData"
    :activity="activityData"
    :loading="loading"
    :error="error"
  />
</template>
