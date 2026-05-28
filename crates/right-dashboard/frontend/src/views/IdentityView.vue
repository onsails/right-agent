<script setup lang="ts">
import { computed } from 'vue'
import AsyncState from '../components/AsyncState.vue'
import { identityLabel, identityTone } from '../components/identityLabels'
import type { IdentityFileSummary, IdentityResponse } from '../types'

const props = defineProps<{
  identity: IdentityResponse | null
  selectedFile: IdentityFileSummary | null
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  selectFile: [name: string]
  refresh: []
}>()

const files = computed(() => props.identity?.files ?? [])
const unreachable = computed(() => files.value.some((f) => f.source === 'sandbox_unreachable'))
</script>

<template>
  <section class="two-column wide-main">
    <section class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Identity</p>
          <h2>{{ identity?.agent ?? 'Agent' }}</h2>
        </div>
        <button v-if="unreachable" type="button" class="tool-button" @click="emit('refresh')">
          Retry
        </button>
      </header>
      <p v-if="identity?.warning" class="notice inline">{{ identity.warning }}</p>
      <div class="row-list">
        <button
          v-for="file in files"
          :key="file.name"
          type="button"
          class="data-row"
          :class="{ selected: selectedFile?.name === file.name }"
          @click="emit('selectFile', file.name)"
        >
          <span class="row-main"><strong>{{ file.name }}</strong></span>
          <span class="row-side">
            <span class="status-pill" :class="identityTone(file.source)">
              {{ identityLabel(file.source) }}
            </span>
          </span>
        </button>
        <p v-if="files.length === 0 && !loading" class="muted-line">No identity files</p>
      </div>
    </section>

    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">File</p>
          <h2>{{ selectedFile?.name ?? 'None selected' }}</h2>
        </div>
        <span
          v-if="selectedFile"
          class="status-pill"
          :class="identityTone(selectedFile.source)"
        >{{ identityLabel(selectedFile.source) }}</span>
      </header>
      <AsyncState :loading="loading" :error="error" :empty="!selectedFile" empty-text="No file selected">
        <template v-if="selectedFile">
          <p class="muted-line">{{ selectedFile.path }}</p>
          <pre v-if="selectedFile.exists && selectedFile.content_preview !== null">{{ selectedFile.content_preview }}<template v-if="selectedFile.truncated">
... truncated
</template></pre>
          <p v-else class="muted-line">{{ identityLabel(selectedFile.source) }}</p>
        </template>
      </AsyncState>
    </aside>
  </section>
</template>
