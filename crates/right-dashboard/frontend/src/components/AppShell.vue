<script setup lang="ts">
import { shortDate } from '../format'
import type { DashboardDisplayMode } from '../telegram'
import StatusPill from './StatusPill.vue'

export interface ShellTab {
  key: string
  label: string
  enabled: boolean
}

defineProps<{
  agent: string
  connectionState: string
  message: string
  lastUpdatedAt: string | null
  tabs: ShellTab[]
  activeTab: string
  displayMode: DashboardDisplayMode
}>()

const emit = defineEmits<{
  select: [tab: string]
  toggleDisplayMode: []
}>()
</script>

<template>
  <main class="app-shell" :class="`display-${displayMode}`">
    <header class="topbar">
      <div>
        <p class="eyebrow">Right Agent</p>
        <h1>{{ agent }}</h1>
      </div>
      <div class="topbar-actions">
        <button
          type="button"
          class="display-mode-button"
          :aria-label="displayMode === 'fullscreen' ? 'Use normal view' : 'Use fullscreen view'"
          @click="emit('toggleDisplayMode')"
        >
          {{ displayMode === 'fullscreen' ? 'Normal' : 'Fullscreen' }}
        </button>
        <StatusPill :status="connectionState" />
      </div>
    </header>

    <section v-if="connectionState !== 'live'" class="notice" :class="connectionState">
      <strong>{{ message }}</strong>
      <span v-if="lastUpdatedAt">Last update {{ shortDate(lastUpdatedAt) }}</span>
    </section>

    <nav class="view-tabs" aria-label="Dashboard views">
      <button
        v-for="tab in tabs"
        v-show="tab.enabled"
        :key="tab.key"
        type="button"
        class="tab-button"
        :class="{ active: activeTab === tab.key }"
        @click="emit('select', tab.key)"
      >
        {{ tab.label }}
      </button>
    </nav>

    <slot />
  </main>
</template>
