<script setup lang="ts">
import StatusPill from '../components/StatusPill.vue'
import type { IdentityFileSummary, IdentityResponse } from '../types'

defineProps<{
  identity: IdentityResponse | null
  selectedFile: IdentityFileSummary | null
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  selectFile: [name: string]
}>()
</script>

<template>
  <section class="two-column wide-main">
    <section class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Identity</p>
          <h2>{{ identity?.agent ?? 'Agent' }}</h2>
        </div>
        <StatusPill :status="identity?.source ?? 'unavailable'" />
      </header>
      <p v-if="identity?.warning" class="notice inline">{{ identity.warning }}</p>
      <div class="segmented">
        <button
          v-for="file in identity?.files ?? []"
          :key="file.name"
          type="button"
          class="segment-button"
          :class="{ active: selectedFile?.name === file.name }"
          @click="emit('selectFile', file.name)"
        >
          {{ file.name }}
        </button>
      </div>
      <dl class="meta-grid compact">
        <div v-for="file in identity?.files ?? []" :key="file.name">
          <dt>{{ file.name }}</dt>
          <dd><StatusPill :status="file.source" :label="file.exists ? file.source : 'missing'" /></dd>
        </div>
      </dl>
    </section>

    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">File</p>
          <h2>{{ selectedFile?.name ?? 'None selected' }}</h2>
        </div>
        <StatusPill v-if="selectedFile" :status="selectedFile.source" />
      </header>
      <p v-if="loading" class="muted-line">Loading</p>
      <p v-else-if="error" class="notice inline">{{ error }}</p>
      <p v-else-if="!selectedFile" class="muted-line">No file selected</p>
      <template v-if="selectedFile">
        <p class="muted-line">{{ selectedFile.path }}</p>
        <pre v-if="selectedFile.exists">{{ selectedFile.content_preview }}<template v-if="selectedFile.truncated">
... truncated
</template></pre>
        <p v-else class="muted-line">Missing</p>
      </template>
    </aside>
  </section>
</template>
