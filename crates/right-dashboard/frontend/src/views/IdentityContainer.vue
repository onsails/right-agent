<script setup lang="ts">
import { ref, watch } from 'vue'

import { DashboardApiError, identityFile, identityFiles } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import IdentityView from './IdentityView.vue'
import type { IdentityFileSummary } from '../types'

const { data: identity, refresh } = useLiveResource(identityFiles, { key: 'identity', intervalMs: 30000 })

const selectedFile = ref<IdentityFileSummary | null>(null)
const loadingFile = ref(false)
const fileError = ref<string | null>(null)

watch(identity, (value) => {
  if (selectedFile.value === null && value !== null) {
    selectedFile.value = value.files[0] ?? null
  }
})

async function selectFile(name: string): Promise<void> {
  loadingFile.value = true
  fileError.value = null
  try {
    const response = await identityFile(name)
    selectedFile.value = response.file
    if (identity.value !== null) {
      identity.value.warning = response.warning ?? identity.value.warning
      identity.value.files = identity.value.files.map((file) => (file.name === name ? response.file : file))
    }
  } catch (err) {
    if (err instanceof DashboardApiError && err.isLocked) {
      void refresh()
    }
    fileError.value = err instanceof Error ? err.message : 'Identity file unavailable'
  } finally {
    loadingFile.value = false
  }
}
</script>

<template>
  <IdentityView
    :identity="identity"
    :selected-file="selectedFile"
    :loading="loadingFile"
    :error="fileError"
    @select-file="selectFile"
    @refresh="refresh"
  />
</template>
