<script setup lang="ts">
import { doctorStatus, sandboxStats } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import HealthView from './HealthView.vue'

const manual = { immediate: false, intervalMs: 0, reportConnection: false }

const { data: doctor, loading: loadingDoctor, error: doctorError, refresh: refreshDoctor } =
  useLiveResource(doctorStatus, { ...manual, key: 'doctor' })
const { data: sandbox, loading: loadingSandbox, error: sandboxError, refresh: refreshSandbox } =
  useLiveResource(sandboxStats, { ...manual, key: 'sandbox' })
</script>

<template>
  <HealthView
    :doctor="doctor"
    :sandbox="sandbox"
    :loading-doctor="loadingDoctor"
    :loading-sandbox="loadingSandbox"
    :doctor-error="doctorError"
    :sandbox-error="sandboxError"
    @refresh-doctor="refreshDoctor"
    @refresh-sandbox="refreshSandbox"
  />
</template>
