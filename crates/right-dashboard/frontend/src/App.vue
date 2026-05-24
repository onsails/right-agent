<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import {
  DashboardApiError,
  bootstrap,
  dashboardOverview,
  doctorStatus,
  identityFile,
  identityFiles,
  learningEpisodeDetail,
  learningEpisodes,
  learningOverview,
  learningReportDetail,
  overview as activityOverview,
  runDetail,
  sandboxStats,
  skillDetail,
  skillsOverview,
  usageOverview,
} from './api'
import { initializeTelegramWebApp } from './telegram'
import AppShell from './components/AppShell.vue'
import ActivityView from './views/ActivityView.vue'
import HealthView from './views/HealthView.vue'
import IdentityView from './views/IdentityView.vue'
import KnowledgeView from './views/KnowledgeView.vue'
import OverviewView from './views/OverviewView.vue'
import UsageView from './views/UsageView.vue'
import type {
  BootstrapResponse, DashboardOverviewResponse, DoctorResponse, IdentityFileSummary, IdentityResponse,
  LearningEpisodeDetailResponse, LearningEpisodeSummary, LearningEpisodesResponse, LearningOverviewResponse,
  LearningReportDetailResponse, LearningReportSummary, OverviewResponse, RunDetailResponse, RunSummary,
  SandboxStatsResponse, SkillDetailResponse, SkillSummary, SkillsResponse, UsageOverviewResponse,
} from './types'

type ConnectionState = 'loading' | 'live' | 'stale' | 'offline' | 'locked'
type DashboardTab = 'overview' | 'activity' | 'knowledge' | 'usage' | 'identity' | 'health'
type KnowledgeTab = 'episodes' | 'reports' | 'skills'

const bootstrapData = ref<BootstrapResponse | null>(null)
const dashboardData = ref<DashboardOverviewResponse | null>(null)
const activityData = ref<OverviewResponse | null>(null)
const usageData = ref<UsageOverviewResponse | null>(null)
const learningData = ref<LearningOverviewResponse | null>(null)
const learningEpisodesData = ref<LearningEpisodesResponse | null>(null)
const skillsData = ref<SkillsResponse | null>(null)
const identityData = ref<IdentityResponse | null>(null)
const doctorData = ref<DoctorResponse | null>(null)
const sandboxData = ref<SandboxStatsResponse | null>(null)

const selectedRun = ref<RunDetailResponse | null>(null)
const selectedRunId = ref<string | null>(null)
const selectedLearningReport = ref<LearningReportDetailResponse | null>(null)
const selectedLearningReportId = ref<number | null>(null)
const selectedEpisode = ref<LearningEpisodeDetailResponse | null>(null)
const selectedEpisodeId = ref<number | null>(null)
const selectedSkill = ref<SkillDetailResponse | null>(null)
const selectedSkillName = ref<string | null>(null)
const selectedIdentityFile = ref<IdentityFileSummary | null>(null)

const activeTab = ref<DashboardTab>('overview')
const activeKnowledgeTab = ref<KnowledgeTab>('episodes')
const connectionState = ref<ConnectionState>('loading')
const message = ref('Loading dashboard')
const lastUpdatedAt = ref<string | null>(null)

const detailError = ref<string | null>(null)
const reportError = ref<string | null>(null)
const episodeError = ref<string | null>(null)
const skillError = ref<string | null>(null)
const identityError = ref<string | null>(null)
const doctorError = ref<string | null>(null)
const sandboxError = ref<string | null>(null)

const loadingDetail = ref(false)
const loadingReport = ref(false)
const loadingEpisode = ref(false)
const loadingSkill = ref(false)
const loadingIdentity = ref(false)
const loadingDoctor = ref(false)
const loadingSandbox = ref(false)

let pollTimer: number | undefined

const shellTitle = computed(() => bootstrapData.value?.agent ?? dashboardData.value?.agent ?? 'Dashboard')
const refreshIntervalMs = computed(() => Math.max(bootstrapData.value?.refresh_interval_secs ?? 5, 1) * 1000)
const tabs = computed(() => {
  const features = bootstrapData.value?.features
  return [
    { key: 'overview', label: 'Overview', enabled: true },
    { key: 'activity', label: 'Activity', enabled: features?.activity ?? true },
    { key: 'knowledge', label: 'Knowledge', enabled: (features?.knowledge_learning ?? true) || (features?.knowledge_skills ?? true) },
    { key: 'usage', label: 'Usage', enabled: features?.usage ?? true },
    { key: 'identity', label: 'Identity', enabled: features?.identity ?? true },
    { key: 'health', label: 'Health', enabled: (features?.doctor ?? true) || (features?.sandbox_stats ?? true) },
  ]
})

onMounted(() => {
  initializeTelegramWebApp()
  void loadInitial()
})

onBeforeUnmount(() => {
  if (pollTimer !== undefined) {
    window.clearInterval(pollTimer)
  }
})

async function loadInitial(): Promise<void> {
  try {
    bootstrapData.value = await bootstrap()
    await refreshOverview()
    await refreshActivity()
    schedulePolling()
  } catch (error) {
    applyErrorState(error)
  }
}

function schedulePolling(): void {
  if (pollTimer !== undefined) {
    window.clearInterval(pollTimer)
  }
  pollTimer = window.setInterval(() => {
    void refreshOverview()
    if (activeTab.value === 'activity' || activeTab.value === 'overview') {
      void refreshActivity()
    }
  }, refreshIntervalMs.value)
}

function setActiveTab(tab: string): void {
  if (!isDashboardTab(tab)) {
    return
  }
  activeTab.value = tab
  void refreshActiveTab()
}

function setKnowledgeTab(tab: KnowledgeTab): void {
  activeKnowledgeTab.value = tab
  void refreshKnowledge()
}

function isDashboardTab(tab: string): tab is DashboardTab {
  return ['overview', 'activity', 'knowledge', 'usage', 'identity', 'health'].includes(tab)
}

async function refreshActiveTab(): Promise<void> {
  if (activeTab.value === 'overview') {
    await refreshOverview()
    await refreshActivity()
  } else if (activeTab.value === 'activity') {
    await refreshActivity()
  } else if (activeTab.value === 'knowledge') {
    await refreshKnowledge()
  } else if (activeTab.value === 'usage') {
    await refreshUsage()
  } else if (activeTab.value === 'identity') {
    await refreshIdentity()
  }
}

async function guarded(load: () => Promise<void>): Promise<void> {
  try {
    await load()
    connectionState.value = 'live'
    message.value = 'Live'
    lastUpdatedAt.value = new Date().toISOString()
  } catch (error) {
    applyErrorState(error)
  }
}

async function refreshOverview(): Promise<void> {
  await guarded(async () => {
    dashboardData.value = await dashboardOverview()
  })
}

async function refreshActivity(): Promise<void> {
  await guarded(async () => {
    const data = await activityOverview()
    activityData.value = data
    if (selectedRunId.value !== null) {
      const stillPresent = data.crons.some((cron) => cron.recent_runs.some((run) => run.id === selectedRunId.value))
      if (!stillPresent) {
        selectedRunId.value = null
        selectedRun.value = null
        detailError.value = null
      }
    }
  })
}

async function refreshUsage(): Promise<void> {
  await guarded(async () => {
    usageData.value = await usageOverview()
  })
}

async function refreshKnowledge(): Promise<void> {
  if (activeKnowledgeTab.value === 'skills') {
    await refreshSkills()
    return
  }

  await guarded(async () => {
    if (activeKnowledgeTab.value === 'episodes') {
      const [overviewData, episodes] = await Promise.all([learningOverview(), learningEpisodes()])
      learningData.value = overviewData
      learningEpisodesData.value = episodes
    } else {
      learningData.value = await learningOverview()
    }
  })
}

async function refreshSkills(): Promise<void> {
  await guarded(async () => {
    skillsData.value = await skillsOverview()
  })
}

async function refreshIdentity(): Promise<void> {
  await guarded(async () => {
    identityData.value = await identityFiles()
    if (selectedIdentityFile.value === null) {
      selectedIdentityFile.value = identityData.value.files[0] ?? null
    }
  })
}

async function refreshDoctor(): Promise<void> {
  loadingDoctor.value = true
  doctorError.value = null
  try {
    doctorData.value = await doctorStatus()
  } catch (error) {
    doctorError.value = error instanceof Error ? error.message : 'Doctor unavailable'
    applyErrorState(error)
  } finally {
    loadingDoctor.value = false
  }
}

async function refreshSandbox(): Promise<void> {
  loadingSandbox.value = true
  sandboxError.value = null
  try {
    sandboxData.value = await sandboxStats()
  } catch (error) {
    sandboxError.value = error instanceof Error ? error.message : 'Sandbox unavailable'
    applyErrorState(error)
  } finally {
    loadingSandbox.value = false
  }
}

function applyErrorState(error: unknown): void {
  if (error instanceof DashboardApiError && error.isLocked) {
    connectionState.value = 'locked'
    message.value = error.message
    return
  }
  connectionState.value = dashboardData.value !== null || activityData.value !== null ? 'stale' : 'offline'
  message.value = error instanceof Error ? error.message : 'Dashboard unavailable'
}

async function selectRun(run: RunSummary): Promise<void> {
  const runId = run.id
  selectedRunId.value = runId
  selectedRun.value = null
  loadingDetail.value = true
  detailError.value = null
  try {
    const detail = await runDetail(runId)
    if (selectedRunId.value === runId) {
      selectedRun.value = detail
    }
  } catch (error) {
    if (error instanceof DashboardApiError && error.isLocked) {
      applyErrorState(error)
    }
    if (selectedRunId.value === runId) {
      detailError.value = error instanceof Error ? error.message : 'Run unavailable'
    }
  } finally {
    if (selectedRunId.value === runId) {
      loadingDetail.value = false
    }
  }
}

async function selectLearningReport(report: LearningReportSummary): Promise<void> {
  const reportId = report.id
  selectedLearningReportId.value = reportId
  selectedLearningReport.value = null
  loadingReport.value = true
  reportError.value = null
  try {
    const detail = await learningReportDetail(reportId)
    if (selectedLearningReportId.value === reportId) {
      selectedLearningReport.value = detail
    }
  } catch (error) {
    if (error instanceof DashboardApiError && error.isLocked) {
      applyErrorState(error)
    }
    if (selectedLearningReportId.value === reportId) {
      reportError.value = error instanceof Error ? error.message : 'Report unavailable'
    }
  } finally {
    if (selectedLearningReportId.value === reportId) {
      loadingReport.value = false
    }
  }
}

async function selectEpisode(episode: LearningEpisodeSummary): Promise<void> {
  selectedEpisodeId.value = episode.id
  selectedEpisode.value = null
  loadingEpisode.value = true
  episodeError.value = null
  try {
    const detail = await learningEpisodeDetail(episode.id)
    if (selectedEpisodeId.value === episode.id) {
      selectedEpisode.value = detail
    }
  } catch (error) {
    if (error instanceof DashboardApiError && error.isLocked) {
      applyErrorState(error)
    }
    if (selectedEpisodeId.value === episode.id) {
      episodeError.value = error instanceof Error ? error.message : 'Episode unavailable'
    }
  } finally {
    if (selectedEpisodeId.value === episode.id) {
      loadingEpisode.value = false
    }
  }
}

async function selectSkill(skill: SkillSummary): Promise<void> {
  selectedSkillName.value = skill.name
  selectedSkill.value = null
  loadingSkill.value = true
  skillError.value = null
  try {
    const detail = await skillDetail(skill.name)
    if (selectedSkillName.value === skill.name) {
      selectedSkill.value = detail
    }
  } catch (error) {
    if (error instanceof DashboardApiError && error.isLocked) {
      applyErrorState(error)
    }
    if (selectedSkillName.value === skill.name) {
      skillError.value = error instanceof Error ? error.message : 'Skill unavailable'
    }
  } finally {
    if (selectedSkillName.value === skill.name) {
      loadingSkill.value = false
    }
  }
}

function applySkillPinned({ skillName, pinned }: { skillName: string, pinned: boolean }): void {
  if (selectedSkill.value && selectedSkill.value.skill.name === skillName) {
    selectedSkill.value = {
      ...selectedSkill.value,
      skill: { ...selectedSkill.value.skill, pinned },
    }
  }
  const current = skillsData.value
  if (current === null) {
    return
  }
  const updateGroup = (group: SkillSummary[]): SkillSummary[] =>
    group.map((skill) => (skill.name === skillName ? { ...skill, pinned } : skill))
  skillsData.value = {
    ...current,
    groups: {
      core: updateGroup(current.groups.core),
      learned: updateGroup(current.groups.learned),
      other: updateGroup(current.groups.other),
    },
  }
}

async function selectIdentityFile(name: string): Promise<void> {
  loadingIdentity.value = true
  identityError.value = null
  try {
    const response = await identityFile(name)
    selectedIdentityFile.value = response.file
    if (identityData.value !== null) {
      identityData.value.warning = response.warning ?? identityData.value.warning
      identityData.value.files = identityData.value.files.map((file) => file.name === name ? response.file : file)
    }
  } catch (error) {
    if (error instanceof DashboardApiError && error.isLocked) {
      applyErrorState(error)
    }
    identityError.value = error instanceof Error ? error.message : 'Identity file unavailable'
  } finally {
    loadingIdentity.value = false
  }
}
</script>

<template>
  <AppShell
    :agent="shellTitle"
    :connection-state="connectionState"
    :message="message"
    :last-updated-at="lastUpdatedAt"
    :tabs="tabs"
    :active-tab="activeTab"
    @select="setActiveTab"
  >
    <OverviewView v-if="activeTab === 'overview'" :overview="dashboardData" :activity="activityData" />
    <ActivityView
      v-else-if="activeTab === 'activity'"
      :overview="activityData"
      :selected-run="selectedRun"
      :selected-run-id="selectedRunId"
      :loading-detail="loadingDetail"
      :detail-error="detailError"
      @select-run="selectRun"
    />
    <KnowledgeView
      v-else-if="activeTab === 'knowledge'"
      :active-subtab="activeKnowledgeTab"
      :learning="learningData"
      :episodes="learningEpisodesData"
      :selected-episode="selectedEpisode"
      :selected-episode-id="selectedEpisodeId"
      :selected-report="selectedLearningReport"
      :selected-report-id="selectedLearningReportId"
      :selected-skill="selectedSkill"
      :selected-skill-name="selectedSkillName"
      :skills="skillsData"
      :loading-episode="loadingEpisode"
      :loading-report="loadingReport"
      :loading-skill="loadingSkill"
      :episode-error="episodeError"
      :report-error="reportError"
      :skill-error="skillError"
      @set-subtab="setKnowledgeTab"
      @select-episode="selectEpisode"
      @select-report="selectLearningReport"
      @select-skill="selectSkill"
      @skill-pinned="applySkillPinned"
    />
    <UsageView v-else-if="activeTab === 'usage'" :usage="usageData" />
    <IdentityView
      v-else-if="activeTab === 'identity'"
      :identity="identityData"
      :selected-file="selectedIdentityFile"
      :loading="loadingIdentity"
      :error="identityError"
      @select-file="selectIdentityFile"
    />
    <HealthView
      v-else
      :doctor="doctorData"
      :sandbox="sandboxData"
      :loading-doctor="loadingDoctor"
      :loading-sandbox="loadingSandbox"
      :doctor-error="doctorError"
      :sandbox-error="sandboxError"
      @refresh-doctor="refreshDoctor"
      @refresh-sandbox="refreshSandbox"
    />
  </AppShell>
</template>

<style>
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
