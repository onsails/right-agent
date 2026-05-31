<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'

import { bootstrap } from './api'
import { useLiveResource } from './composables/useLiveResource'
import { provideLiveConfig } from './composables/liveConfig'
import { globalConnectionState, globalLastUpdatedAt } from './composables/liveStatus'
import {
  dashboardTabItems,
  isDashboardTab,
  normalizeInitialTab,
  type DashboardTab,
} from './dashboardTabs'
import { initialDashboardTabFromLocation } from './format'
import {
  applyTelegramDisplayMode,
  initializeTelegramWebApp,
  nextDashboardDisplayModePreference,
  readDashboardDisplayMode,
  subscribeTelegramFullscreenChanges,
  type DashboardDisplayMode,
} from './telegram'
import AppShell from './components/AppShell.vue'
import OverviewContainer from './views/OverviewContainer.vue'
import ActivityContainer from './views/ActivityContainer.vue'
import KnowledgeContainer from './views/KnowledgeContainer.vue'
import UsageContainer from './views/UsageContainer.vue'
import IdentityContainer from './views/IdentityContainer.vue'
import HealthContainer from './views/HealthContainer.vue'
import McpView from './views/McpView.vue'
import ProvidersView from './views/ProvidersView.vue'

const { data: bootstrapData } = useLiveResource(bootstrap, { intervalMs: 0, key: 'bootstrap' })

const activeTab = ref<DashboardTab>(
  normalizeInitialTab(initialDashboardTabFromLocation(window.location.search, window.location.hash)),
)
const preferredDisplayMode = ref<DashboardDisplayMode>(readDashboardDisplayMode())
const displayMode = ref<DashboardDisplayMode>('normal')

const refreshIntervalMs = computed(() => Math.max(bootstrapData.value?.refresh_interval_secs ?? 5, 1) * 1000)
provideLiveConfig(computed(() => ({ intervalMs: refreshIntervalMs.value })))

const shellTitle = computed(() => bootstrapData.value?.agent ?? 'Dashboard')
const tabs = computed(() => dashboardTabItems(bootstrapData.value?.features))

const connectionMessage = computed(() => {
  switch (globalConnectionState.value) {
    case 'live':
      return 'Live'
    case 'stale':
      return 'Reconnecting'
    case 'offline':
      return 'Dashboard unavailable'
    case 'locked':
      return 'Dashboard locked'
    default:
      return 'Loading dashboard'
  }
})

let unsubscribeTelegramFullscreen: (() => void) | undefined

onMounted(() => {
  const webApp = window.Telegram?.WebApp
  displayMode.value = initializeTelegramWebApp(webApp, preferredDisplayMode.value)
  unsubscribeTelegramFullscreen = subscribeTelegramFullscreenChanges(webApp, (mode) => {
    displayMode.value = mode
  })
})

onBeforeUnmount(() => {
  unsubscribeTelegramFullscreen?.()
})

function setActiveTab(tab: string): void {
  if (!isDashboardTab(tab)) {
    return
  }
  activeTab.value = tab
}

function toggleDisplayMode(): void {
  const nextMode = nextDashboardDisplayModePreference(preferredDisplayMode.value)
  preferredDisplayMode.value = nextMode
  displayMode.value = applyTelegramDisplayMode(nextMode)
}
</script>

<template>
  <AppShell
    :agent="shellTitle"
    :connection-state="globalConnectionState"
    :message="connectionMessage"
    :last-updated-at="globalLastUpdatedAt"
    :tabs="tabs"
    :active-tab="activeTab"
    :display-mode="displayMode"
    :preferred-display-mode="preferredDisplayMode"
    @select="setActiveTab"
    @toggle-display-mode="toggleDisplayMode"
  >
    <OverviewContainer v-if="activeTab === 'overview'" />
    <ActivityContainer v-else-if="activeTab === 'activity'" />
    <KnowledgeContainer v-else-if="activeTab === 'knowledge'" />
    <UsageContainer v-else-if="activeTab === 'usage'" />
    <IdentityContainer v-else-if="activeTab === 'identity'" />
    <McpView v-else-if="activeTab === 'mcp'" />
    <ProvidersView v-else-if="activeTab === 'providers'" />
    <HealthContainer v-else-if="activeTab === 'health'" />
    <section v-else class="empty-panel">Unknown dashboard view</section>
  </AppShell>
</template>

<style>
:root {
  --token-input: #6b7b88;
  --token-output: #2481cc;
  --token-create: #b87900;
  --token-read: #0d7a45;
}

* {
  box-sizing: border-box;
}

body {
  margin: 0;
  min-width: 320px;
  color: var(--tg-theme-text-color, #17212b);
  background: var(--tg-theme-bg-color, #f4f6f8);
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  letter-spacing: 0;
}

button {
  font: inherit;
}

.app-shell {
  width: min(1160px, 100%);
  margin: 0 auto;
  padding: calc(14px + env(safe-area-inset-top)) 12px calc(20px + env(safe-area-inset-bottom));
}

.topbar,
.panel-head,
.data-row,
.metric-grid,
.two-column,
.meta-grid {
  display: grid;
  gap: 8px;
}

.topbar {
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  margin-bottom: 12px;
}

.topbar > div,
.panel-head > div,
.row-main,
.row-side {
  min-width: 0;
}

.eyebrow {
  margin: 0 0 2px;
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.72rem;
  font-weight: 700;
  text-transform: uppercase;
}

h1,
h2,
h3,
p {
  margin: 0;
}

h1 {
  font-size: 1.35rem;
  line-height: 1.15;
  overflow-wrap: anywhere;
}

h2 {
  font-size: 1rem;
  line-height: 1.2;
  overflow-wrap: anywhere;
}

h3 {
  font-size: 0.84rem;
}

.status-pill {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-height: 24px;
  padding: 3px 8px;
  border-radius: 999px;
  background: var(--tg-theme-secondary-bg-color, #e8edf1);
  color: var(--tg-theme-hint-color, #546675);
  font-size: 0.72rem;
  font-weight: 700;
  text-transform: uppercase;
  white-space: nowrap;
}

.status-pill.ok {
  color: #0d7a45;
  background: #dff5e8;
}

.status-pill.active {
  color: #8a5a00;
  background: #fff0c2;
}

.status-pill.bad {
  color: #a42323;
  background: #ffe1de;
}

.status-pill.muted {
  color: var(--tg-theme-hint-color, #546675);
  background: var(--tg-theme-secondary-bg-color, #e8edf1);
}

.notice {
  display: grid;
  gap: 3px;
  margin-bottom: 10px;
  padding: 9px 10px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 8px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
  font-size: 0.82rem;
}

.notice span,
.muted-line,
dt,
.row-main small,
.row-side small {
  color: var(--tg-theme-hint-color, #6b7b88);
}

.notice.inline {
  margin: 0;
}

.view-tabs,
.subtabs,
.segmented {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 10px;
}

.view-tabs {
  overflow-x: auto;
}

.display-mode-button,
.tab-button,
.segment-button,
.tool-button {
  min-height: 32px;
  padding: 5px 10px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
}

.tab-button.active,
.segment-button.active {
  border-color: var(--tg-theme-button_color, #2481cc);
  color: var(--tg-theme-button_color, #2481cc);
  font-weight: 700;
}

.topbar-actions {
  display: flex;
  min-width: 0;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  justify-content: flex-end;
}

.display-mode-button {
  flex: 0 1 auto;
  max-width: 100%;
  overflow: hidden;
  font-size: 0.78rem;
  font-weight: 700;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.metric-grid {
  grid-template-columns: repeat(6, minmax(0, 1fr));
  margin-bottom: 10px;
}

.metric-card,
.panel,
.empty-panel {
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.16));
  border-radius: 8px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
}

.metric-card {
  min-height: 70px;
  padding: 9px;
}

.metric-card span {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.75rem;
}

.metric-card strong {
  display: block;
  margin-top: 6px;
  font-size: 1.16rem;
  line-height: 1.1;
  overflow-wrap: anywhere;
}

.metric-card.ok strong {
  color: #0d7a45;
}

.metric-card.active strong {
  color: #8a5a00;
}

.metric-card.bad strong {
  color: #a42323;
}

.two-column {
  grid-template-columns: minmax(0, 1fr) minmax(320px, 0.85fr);
  align-items: start;
}

.two-column.wide-main {
  grid-template-columns: minmax(0, 1fr) minmax(320px, 0.72fr);
}

.panel,
.empty-panel {
  padding: 11px;
}

.panel-head {
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: start;
  margin-bottom: 10px;
}

.panel-head-actions {
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.cron-delete {
  font-size: 0.75rem;
  padding: 0.2rem 0.5rem;
  border-radius: 0.4rem;
  border: 1px solid var(--danger, #c0392b);
  color: var(--danger, #c0392b);
  background: transparent;
  cursor: pointer;
}

.cron-sort {
  display: flex;
  justify-content: flex-end;
  margin-bottom: 0.5rem;
}

.cron-sort-select {
  margin-left: 0.4rem;
}

.list-stack,
.row-list,
.text-block,
.evidence-list {
  display: grid;
  gap: 8px;
}

.meta-grid {
  grid-template-columns: repeat(4, minmax(0, 1fr));
  margin: 0 0 10px;
}

.meta-grid.compact {
  grid-template-columns: repeat(2, minmax(0, 1fr));
}

dt {
  margin-bottom: 2px;
  font-size: 0.75rem;
}

dd {
  margin: 0;
  font-size: 0.82rem;
  font-weight: 650;
  overflow-wrap: anywhere;
}

.data-row {
  width: 100%;
  min-height: 42px;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  padding: 7px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
  text-align: left;
}

.data-row.tall {
  min-height: 58px;
}

.data-row.static {
  cursor: default;
}

.data-row.selected {
  border-color: var(--tg-theme-button_color, #2481cc);
  box-shadow: inset 0 0 0 1px var(--tg-theme-button_color, #2481cc);
}

.row-main,
.row-side {
  display: flex;
  gap: 6px;
  align-items: center;
}

.row-main > strong,
.row-main > span {
  min-width: 0;
  overflow-wrap: anywhere;
}

.row-main {
  flex-wrap: wrap;
}

.row-side {
  flex-direction: column;
  align-items: flex-end;
  gap: 1px;
  font-size: 0.78rem;
  font-weight: 700;
}

.status-dot {
  width: 8px;
  height: 8px;
  flex: 0 0 auto;
  border-radius: 50%;
  background: var(--tg-theme-hint-color, #6b7b88);
}

.status-dot.ok {
  background: #0d7a45;
}

.status-dot.active {
  background: #b87900;
}

.status-dot.bad {
  background: #b92b27;
}

.run-delivery-badge {
  display: inline-flex;
  align-items: center;
  min-height: 22px;
  padding: 2px 7px;
  border-radius: 999px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-hint-color, #546675);
  font-size: 0.7rem;
  font-weight: 700;
  text-transform: uppercase;
  white-space: nowrap;
}

.run-delivery-badge.ok {
  color: #0d7a45;
  background: #dff5e8;
}

.run-delivery-badge.active {
  color: #8a5a00;
  background: #fff0c2;
}

.run-delivery-badge.bad {
  color: #a42323;
  background: #ffe1de;
}

.run-note-preview {
  flex-basis: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.run-inline-detail {
  display: none;
}

.detail-panel {
  position: sticky;
  top: 10px;
  display: grid;
  gap: 10px;
}

.chart-panel {
  display: grid;
  gap: 8px;
  align-content: start;
  min-height: 220px;
}

.two-column + .chart-panel {
  margin-top: 10px;
}

.dashboard-chart {
  width: 100%;
  height: 240px;
}

.chart-empty {
  display: grid;
  min-height: 180px;
  place-items: center;
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.84rem;
}

.marker-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-top: 8px;
}

.marker-chip {
  min-width: 0;
  min-height: 28px;
  padding: 4px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
  overflow-wrap: anywhere;
}

.text-block {
  padding-top: 9px;
  border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
}

.text-block p,
.evidence-item p {
  font-size: 0.82rem;
  line-height: 1.35;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.evidence-item {
  display: grid;
  gap: 3px;
  padding: 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
}

.evidence-item strong,
.evidence-item span {
  font-size: 0.74rem;
}

pre {
  max-height: 280px;
  margin: 0;
  padding: 9px;
  overflow: auto;
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  color: var(--tg-theme-text-color, #17212b);
  font: 0.72rem/1.45 ui-monospace, SFMono-Regular, Consolas, monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}

.table-wrap {
  width: 100%;
  overflow: auto;
}

table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.78rem;
}

th,
td {
  padding: 7px 6px;
  border-bottom: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  text-align: left;
  white-space: nowrap;
}

.model-grid {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 6px;
  margin-top: 10px;
}

.model-row {
  display: flex;
  min-width: 0;
  gap: 8px;
  align-items: center;
  justify-content: space-between;
  padding: 7px 8px;
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  font-size: 0.78rem;
}

.model-row span {
  min-width: 0;
  overflow-wrap: anywhere;
}

@media (max-width: 900px) {
  .metric-grid {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }

  .two-column,
  .two-column.wide-main {
    grid-template-columns: minmax(0, 1fr);
  }

  .detail-panel {
    position: static;
  }

  .run-inline-detail {
    display: grid;
    gap: 9px;
    padding: 10px;
    border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
    border-radius: 7px;
    background: var(--tg-theme-bg-color, #f4f6f8);
  }
}

@media (max-width: 560px) {
  .app-shell.display-normal {
    padding-bottom: calc(78px + env(safe-area-inset-bottom));
  }

  .app-shell.display-normal .view-tabs {
    position: fixed;
    right: 0;
    bottom: 0;
    left: 0;
    z-index: 20;
    flex-wrap: nowrap;
    margin-bottom: 0;
    padding: 8px 12px calc(8px + env(safe-area-inset-bottom));
    overflow-x: auto;
    border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
    background: var(--tg-theme-secondary-bg-color, #ffffff);
  }

  .app-shell.display-normal .view-tabs .tab-button {
    flex: 0 0 auto;
    min-width: 82px;
  }

  .metric-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .meta-grid,
  .meta-grid.compact,
  .model-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
</style>
