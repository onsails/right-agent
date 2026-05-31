<script setup lang="ts">
import { computed, ref, watch } from 'vue'

import { DashboardApiError, identityFile, identityFiles } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import IdentityView from './IdentityView.vue'
import type { IdentityFileSummary, IdentityResponse } from '../types'

const { data: identity, refresh } = useLiveResource(identityFiles, { key: 'identity', intervalMs: 30000 })

const selectedFile = ref<IdentityFileSummary | null>(null)
const loadingFile = ref(false)
const fileError = ref<string | null>(null)

const filePatches = ref(new Map<string, IdentityFileSummary>())
const warningPatch = ref<string | null>(null)

const patchedIdentity = computed((): IdentityResponse | null => {
  const base = identity.value
  if (base === null) return null
  if (filePatches.value.size === 0 && warningPatch.value === null) return base
  return {
    ...base,
    warning: warningPatch.value ?? base.warning,
    files: base.files.map((file) => filePatches.value.get(file.name) ?? file),
  }
})

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
    filePatches.value = new Map(filePatches.value).set(name, response.file)
    if (response.warning !== null) {
      warningPatch.value = response.warning
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
    :identity="patchedIdentity"
    :selected-file="selectedFile"
    :loading="loadingFile"
    :error="fileError"
    @select-file="selectFile"
    @refresh="refresh"
  />
</template>
