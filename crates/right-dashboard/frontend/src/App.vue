<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { DashboardApiError, bootstrap, learningOverview, learningReportDetail, overview, runDetail } from './api'
import type {
  BootstrapResponse,
  CronCard,
  LearningOverviewResponse,
  LearningReportDetailResponse,
  LearningReportSummary,
  OverviewResponse,
  RunDetailResponse,
  RunSummary,
} from './types'

type ConnectionState = 'loading' | 'live' | 'stale' | 'offline' | 'locked'
type DashboardView = 'cron' | 'learning'

const bootstrapData = ref<BootstrapResponse | null>(null)
const overviewData = ref<OverviewResponse | null>(null)
const selectedRun = ref<RunDetailResponse | null>(null)
const selectedRunId = ref<string | null>(null)
const activeView = ref<DashboardView>('cron')
const learningData = ref<LearningOverviewResponse | null>(null)
const selectedLearningReport = ref<LearningReportDetailResponse | null>(null)
const selectedLearningReportId = ref<number | null>(null)
const connectionState = ref<ConnectionState>('loading')
const message = ref('Loading dashboard')
const detailError = ref<string | null>(null)
const learningDetailError = ref<string | null>(null)
const lastUpdatedAt = ref<string | null>(null)
const loadingDetail = ref(false)
const loadingLearningDetail = ref(false)
let pollTimer: number | undefined

const refreshIntervalMs = computed(() => {
  const seconds = overviewData.value?.refresh_interval_secs ?? bootstrapData.value?.refresh_interval_secs ?? 5
  return Math.max(seconds, 1) * 1000
})

const shellTitle = computed(() => bootstrapData.value?.agent ?? overviewData.value?.agent ?? 'Dashboard')
const summary = computed(() => overviewData.value?.summary)
const crons = computed(() => overviewData.value?.crons ?? [])
const activeForeground = computed(() => overviewData.value?.active.foreground.length ?? 0)
const activeBackground = computed(() => overviewData.value?.active.background.length ?? 0)
const activeTotal = computed(() => activeForeground.value + activeBackground.value)
const hasOverview = computed(() => overviewData.value !== null)
const learningEnabled = computed(() => bootstrapData.value?.features.learning_metrics === true)
const learningSummary = computed(() => learningData.value)
const learningReports = computed(() => learningData.value?.recent_reports ?? [])

onMounted(() => {
  window.Telegram?.WebApp?.ready?.()
  window.Telegram?.WebApp?.expand?.()
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
    if (!bootstrapData.value.features.learning_metrics && activeView.value === 'learning') {
      activeView.value = 'cron'
    }
    await refreshOverview()
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
  }, refreshIntervalMs.value)
}

async function refreshOverview(): Promise<void> {
  try {
    const data = await overview()
    overviewData.value = data
    if (learningEnabled.value) {
      learningData.value = await learningOverview()
    }
    connectionState.value = 'live'
    message.value = 'Live'
    lastUpdatedAt.value = new Date().toISOString()

    if (selectedRunId.value !== null) {
      const stillPresent = data.crons.some((cron) => cron.recent_runs.some((run) => run.id === selectedRunId.value))
      if (!stillPresent) {
        selectedRunId.value = null
        selectedRun.value = null
        detailError.value = null
      }
    }

    if (selectedLearningReportId.value !== null && learningData.value !== null) {
      const stillPresent = learningData.value.recent_reports.some((report) => report.id === selectedLearningReportId.value)
      if (!stillPresent) {
        selectedLearningReportId.value = null
        selectedLearningReport.value = null
        learningDetailError.value = null
      }
    }
  } catch (error) {
    applyErrorState(error)
  }
}

function applyErrorState(error: unknown): void {
  if (error instanceof DashboardApiError && error.isLocked) {
    connectionState.value = 'locked'
    message.value = error.message
    return
  }

  connectionState.value = hasOverview.value ? 'stale' : 'offline'
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
      selectedRun.value = null
      detailError.value = error instanceof Error ? error.message : 'Run detail unavailable'
    }
  } finally {
    if (selectedRunId.value === runId) {
      loadingDetail.value = false
    }
  }
}

function setView(view: DashboardView): void {
  if (view === 'learning' && !learningEnabled.value) {
    return
  }
  activeView.value = view
}

async function selectLearningReport(report: LearningReportSummary): Promise<void> {
  const reportId = report.id
  selectedLearningReportId.value = reportId
  selectedLearningReport.value = null
  loadingLearningDetail.value = true
  learningDetailError.value = null

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
      selectedLearningReport.value = null
      learningDetailError.value = error instanceof Error ? error.message : 'Learning report unavailable'
    }
  } finally {
    if (selectedLearningReportId.value === reportId) {
      loadingLearningDetail.value = false
    }
  }
}

function cronStatus(cron: CronCard): string {
  const active = cron.recent_runs.find((run) => isActive(run.status))
  if (active) {
    return active.status
  }
  return cron.last_run?.status ?? 'idle'
}

function isActive(status: string): boolean {
  return status === 'queued' || status === 'running'
}

function statusClass(status: string): string {
  if (status === 'success' || status === 'delivered' || status === 'create_candidate' || status === 'update_candidate') {
    return 'ok'
  }
  if (status === 'failed' || status === 'error') {
    return 'bad'
  }
  if (isActive(status) || status === 'pending') {
    return 'active'
  }
  return 'muted'
}

function money(value: number | null | undefined): string {
  return `$${(value ?? 0).toFixed(2)}`
}

function percent(value: number | null | undefined): string {
  if (value === null || value === undefined) {
    return 'none'
  }
  return `${Math.round(value * 100)}%`
}

function shortDate(value: string | null | undefined): string {
  if (!value) {
    return 'none'
  }
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) {
    return value
  }
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}...` : id
}

function notifyText(value: unknown): string | null {
  if (value === null || value === undefined) {
    return null
  }
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}
</script>

<template>
  <main class="app-shell">
    <header class="topbar">
      <div>
        <p class="eyebrow">Right Agent</p>
        <h1>{{ shellTitle }}</h1>
      </div>
      <div class="state-pill" :class="connectionState">
        {{ connectionState }}
      </div>
    </header>

    <section v-if="connectionState !== 'live'" class="notice" :class="connectionState">
      <strong>{{ message }}</strong>
      <span v-if="lastUpdatedAt">Last update {{ shortDate(lastUpdatedAt) }}</span>
    </section>

    <nav class="view-tabs" aria-label="Dashboard views">
      <button
        type="button"
        class="tab-button"
        :class="{ active: activeView === 'cron' }"
        @click="setView('cron')"
      >
        Cron
      </button>
      <button
        v-if="learningEnabled"
        type="button"
        class="tab-button"
        :class="{ active: activeView === 'learning' }"
        @click="setView('learning')"
      >
        Learning
      </button>
    </nav>

    <template v-if="activeView === 'cron'">
      <section class="summary-grid" aria-label="Summary">
        <article class="summary-card">
          <span>Jobs</span>
          <strong>{{ summary?.cron_count ?? 0 }}</strong>
        </article>
        <article class="summary-card">
          <span>Running</span>
          <strong>{{ summary?.active_cron_count ?? 0 }}</strong>
        </article>
        <article class="summary-card">
          <span>Failures</span>
          <strong>{{ summary?.failed_recent_cron_count ?? 0 }}</strong>
        </article>
        <article class="summary-card">
          <span>Today</span>
          <strong>{{ money(summary?.today_cost_usd) }}</strong>
        </article>
      </section>

      <section class="active-strip" aria-label="Active activity">
        <div>
          <span>Foreground</span>
          <strong>{{ activeForeground }}</strong>
        </div>
        <div>
          <span>Background</span>
          <strong>{{ activeBackground }}</strong>
        </div>
        <div>
          <span>Total</span>
          <strong>{{ activeTotal }}</strong>
        </div>
      </section>

      <section class="content-grid">
        <section class="cron-list" aria-label="Cron jobs">
          <article v-if="crons.length === 0" class="empty-panel">
            No cron jobs
          </article>

          <article v-for="cron in crons" :key="cron.job_name" class="cron-card">
            <header class="cron-header">
              <div>
                <h2>{{ cron.job_name }}</h2>
                <p>{{ cron.schedule }}</p>
              </div>
              <span class="status-pill" :class="statusClass(cronStatus(cron))">
                {{ cronStatus(cron) }}
              </span>
            </header>

            <dl class="meta-grid">
              <div>
                <dt>Type</dt>
                <dd>{{ cron.recurring ? 'recurring' : 'one shot' }}</dd>
              </div>
              <div>
                <dt>Next</dt>
                <dd>{{ shortDate(cron.run_at) }}</dd>
              </div>
              <div>
                <dt>Target</dt>
                <dd>{{ cron.target_chat_id ?? 'default' }}<span v-if="cron.target_thread_id">/{{ cron.target_thread_id }}</span></dd>
              </div>
              <div>
                <dt>Budget</dt>
                <dd>{{ money(cron.max_budget_usd) }}</dd>
              </div>
            </dl>

            <div class="runs">
              <div class="runs-head">
                <span>Recent runs</span>
                <strong>{{ money(cron.recent_runs.reduce((total, run) => total + (run.cost_usd ?? 0), 0)) }}</strong>
              </div>

              <button
                v-for="run in cron.recent_runs"
                :key="run.id"
                class="run-row"
                :class="{ selected: selectedRunId === run.id }"
                type="button"
                @click="selectRun(run)"
              >
                <span class="run-main">
                  <span class="status-dot" :class="statusClass(run.status)"></span>
                  <span>{{ run.status }}</span>
                  <small>{{ shortId(run.id) }}</small>
                </span>
                <span class="run-side">
                  <span>{{ money(run.cost_usd) }}</span>
                  <small>{{ shortDate(run.started_at) }}</small>
                </span>
              </button>

              <p v-if="cron.recent_runs.length === 0" class="muted-line">No recent runs</p>
            </div>
          </article>
        </section>

        <aside class="detail-panel" aria-label="Run detail">
          <header>
            <p class="eyebrow">Run detail</p>
            <h2>{{ selectedRun?.run.id ? shortId(selectedRun.run.id) : 'None selected' }}</h2>
          </header>

        <p v-if="loadingDetail" class="muted-line">Loading run detail</p>
        <p v-else-if="detailError" class="notice inline">{{ detailError }}</p>
        <p v-else-if="!selectedRun" class="muted-line">No run selected</p>

        <template v-if="selectedRun">
          <dl class="detail-meta">
            <div>
              <dt>Status</dt>
              <dd><span class="status-pill" :class="statusClass(selectedRun.run.status)">{{ selectedRun.run.status }}</span></dd>
            </div>
            <div>
              <dt>Kind</dt>
              <dd>{{ selectedRun.run.kind }}</dd>
            </div>
            <div>
              <dt>Delivery</dt>
              <dd>{{ selectedRun.run.delivery_status }}</dd>
            </div>
            <div>
              <dt>Exit</dt>
              <dd>{{ selectedRun.run.exit_code ?? 'none' }}</dd>
            </div>
            <div>
              <dt>Started</dt>
              <dd>{{ shortDate(selectedRun.run.started_at) }}</dd>
            </div>
            <div>
              <dt>Finished</dt>
              <dd>{{ shortDate(selectedRun.run.finished_at) }}</dd>
            </div>
            <div>
              <dt>Cost</dt>
              <dd>{{ money(selectedRun.run.cost_usd) }}</dd>
            </div>
          </dl>

          <section class="detail-block">
            <h3>Summary</h3>
            <p>{{ selectedRun.summary || selectedRun.no_notify_reason || 'No summary' }}</p>
          </section>

          <section v-if="notifyText(selectedRun.notify_json)" class="detail-block">
            <h3>Notify</h3>
            <pre>{{ notifyText(selectedRun.notify_json) }}</pre>
          </section>

          <section class="detail-block">
            <h3>Log</h3>
            <p v-if="!selectedRun.log.available" class="muted-line">Log unavailable</p>
            <pre v-else>{{ selectedRun.log.lines.join('\n') }}<template v-if="selectedRun.log.truncated">
... truncated
</template></pre>
          </section>
        </template>
        </aside>
      </section>
    </template>

    <template v-else-if="activeView === 'learning'">
      <section class="learning-funnel" aria-label="Learning funnel">
        <article class="summary-card">
          <span>Signals</span>
          <strong>{{ learningSummary?.funnel.signals_accepted_24h ?? 0 }}</strong>
        </article>
        <article class="summary-card">
          <span>Episodes</span>
          <strong>{{ learningSummary?.funnel.episodes_reviewed_24h ?? 0 }}</strong>
        </article>
        <article class="summary-card">
          <span>Candidates</span>
          <strong>{{ (learningSummary?.funnel.create_candidates_24h ?? 0) + (learningSummary?.funnel.update_candidates_24h ?? 0) }}</strong>
        </article>
        <article class="summary-card">
          <span>Created/updated</span>
          <strong>{{ learningSummary?.funnel.foreground_created_or_updated_7d ?? 0 }}</strong>
        </article>
      </section>

      <section class="learning-grid">
        <section class="cron-list" aria-label="Learning reports">
          <article v-if="learningReports.length === 0" class="empty-panel">
            No learning reports
          </article>

          <button
            v-for="report in learningReports"
            :key="report.id"
            class="learning-report-row"
            :class="{ selected: selectedLearningReportId === report.id }"
            type="button"
            @click="selectLearningReport(report)"
          >
            <span class="run-main">
              <span class="status-dot" :class="statusClass(report.status)"></span>
              <span>{{ report.status }}</span>
              <small>{{ report.confidence }} / {{ report.trigger_kind }}</small>
            </span>
            <span class="report-summary">
              {{ report.candidate_skill_name || report.candidate_summary || 'No candidate' }}
            </span>
          </button>
        </section>

        <aside class="detail-panel" aria-label="Learning metrics">
          <header>
            <p class="eyebrow">Learning quality</p>
            <h2>{{ percent(learningSummary?.quality.candidate_rate) }} candidates</h2>
          </header>

          <dl class="detail-meta">
            <div>
              <dt>Nothing</dt>
              <dd>{{ percent(learningSummary?.quality.nothing_to_learn_rate) }}</dd>
            </div>
            <div>
              <dt>High</dt>
              <dd>{{ learningSummary?.quality.high_confidence_count_24h ?? 0 }}</dd>
            </div>
            <div>
              <dt>Daily</dt>
              <dd>{{ learningSummary?.health.daily_review_count ?? 0 }}/{{ learningSummary?.health.daily_limit ?? 12 }}</dd>
            </div>
            <div>
              <dt>Gate</dt>
              <dd>{{ learningSummary?.health.review_running ? 'running' : 'idle' }}</dd>
            </div>
          </dl>

          <section class="detail-block">
            <h3>Report detail</h3>
            <p v-if="loadingLearningDetail" class="muted-line">Loading learning report</p>
            <p v-else-if="learningDetailError" class="notice inline">{{ learningDetailError }}</p>
            <p v-else-if="!selectedLearningReport" class="muted-line">No report selected</p>

            <template v-if="selectedLearningReport">
              <p>{{ selectedLearningReport.report.candidate_summary || selectedLearningReport.report.status }}</p>
              <p v-if="selectedLearningReport.selector?.boundary_rationale" class="muted-line">
                {{ selectedLearningReport.selector.boundary_rationale }}
              </p>
              <div class="evidence-list">
                <div
                  v-for="snippet in selectedLearningReport.evidence"
                  :key="snippet.ref_id"
                  class="evidence-item"
                >
                  <strong>{{ snippet.ref_id }}</strong>
                  <span>{{ snippet.available ? (snippet.event_kind || snippet.role || snippet.source) : 'unavailable' }}</span>
                  <p>{{ snippet.text || 'Snippet unavailable' }}</p>
                </div>
              </div>
            </template>
          </section>
        </aside>
      </section>
    </template>
  </main>
</template>

<style scoped>
:global(*) {
  box-sizing: border-box;
}

:global(body) {
  margin: 0;
  min-width: 320px;
  color: var(--tg-theme-text-color, #17212b);
  background: var(--tg-theme-bg-color, #f4f6f8);
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  letter-spacing: 0;
}

:global(button) {
  font: inherit;
}

.app-shell {
  width: min(1120px, 100%);
  margin: 0 auto;
  padding: calc(14px + env(safe-area-inset-top)) 12px calc(20px + env(safe-area-inset-bottom));
}

.topbar,
.cron-header,
.runs-head,
.run-row,
.active-strip,
.summary-grid,
.content-grid {
  display: grid;
  gap: 8px;
}

.topbar {
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  margin-bottom: 12px;
}

.topbar > div,
.cron-header > div {
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
}

h1,
.cron-header h2 {
  overflow-wrap: anywhere;
}

h2 {
  font-size: 1rem;
  line-height: 1.2;
}

h3 {
  font-size: 0.82rem;
  line-height: 1.2;
}

.state-pill,
.status-pill {
  display: inline-flex;
  align-items: center;
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

.state-pill.live,
.status-pill.ok {
  color: #0d7a45;
  background: #dff5e8;
}

.state-pill.stale,
.status-pill.active {
  color: #8a5a00;
  background: #fff0c2;
}

.state-pill.offline,
.state-pill.locked,
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

.notice span {
  color: var(--tg-theme-hint-color, #6b7b88);
}

.notice.inline {
  margin: 0;
}

.view-tabs {
  display: flex;
  gap: 6px;
  margin-bottom: 10px;
}

.tab-button {
  min-height: 32px;
  padding: 5px 10px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
}

.tab-button.active {
  border-color: var(--tg-theme-button_color, #2481cc);
  color: var(--tg-theme-button_color, #2481cc);
  font-weight: 700;
}

.summary-grid {
  grid-template-columns: repeat(4, minmax(0, 1fr));
  margin-bottom: 8px;
}

.summary-card,
.cron-card,
.detail-panel,
.active-strip,
.empty-panel {
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.16));
  border-radius: 8px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
}

.summary-card {
  min-height: 70px;
  padding: 9px;
}

.summary-card span,
.active-strip span,
dt,
.runs-head span,
.muted-line {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.75rem;
}

.summary-card strong {
  display: block;
  margin-top: 6px;
  font-size: 1.18rem;
  line-height: 1.1;
}

.active-strip {
  grid-template-columns: repeat(3, minmax(0, 1fr));
  margin-bottom: 10px;
  padding: 9px 10px;
}

.active-strip div {
  min-width: 0;
}

.active-strip strong {
  display: block;
  margin-top: 2px;
  font-size: 1rem;
}

.content-grid {
  grid-template-columns: minmax(0, 1fr) minmax(320px, 0.7fr);
  align-items: start;
}

.learning-funnel {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 8px;
  margin-bottom: 10px;
}

.learning-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(320px, 0.7fr);
  gap: 8px;
  align-items: start;
}

.cron-list {
  display: grid;
  gap: 10px;
}

.cron-card,
.detail-panel,
.empty-panel {
  padding: 11px;
}

.cron-header {
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: start;
  margin-bottom: 10px;
}

.cron-header p {
  margin-top: 3px;
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.8rem;
  overflow-wrap: anywhere;
}

.meta-grid,
.detail-meta {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 8px;
  margin: 0 0 10px;
}

.meta-grid div,
.detail-meta div {
  min-width: 0;
}

dt {
  margin-bottom: 2px;
}

dd {
  margin: 0;
  font-size: 0.82rem;
  font-weight: 650;
  overflow-wrap: anywhere;
}

.runs {
  display: grid;
  gap: 6px;
}

.runs-head,
.run-row {
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
}

.runs-head strong {
  font-size: 0.82rem;
}

.run-row {
  width: 100%;
  min-height: 42px;
  padding: 7px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
  text-align: left;
}

.run-row.selected {
  border-color: var(--tg-theme-button_color, #2481cc);
  box-shadow: inset 0 0 0 1px var(--tg-theme-button_color, #2481cc);
}

.learning-report-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 5px;
  width: 100%;
  min-height: 58px;
  padding: 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
  text-align: left;
}

.learning-report-row.selected {
  border-color: var(--tg-theme-button_color, #2481cc);
  box-shadow: inset 0 0 0 1px var(--tg-theme-button_color, #2481cc);
}

.report-summary {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.78rem;
  overflow-wrap: anywhere;
}

.run-main,
.run-side {
  display: flex;
  min-width: 0;
  gap: 6px;
  align-items: center;
}

.run-main span:nth-child(2) {
  font-weight: 700;
}

.run-main small,
.run-side small {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.72rem;
}

.run-side {
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

.detail-panel {
  position: sticky;
  top: 10px;
  display: grid;
  gap: 10px;
}

.detail-meta {
  grid-template-columns: repeat(2, minmax(0, 1fr));
  margin-bottom: 0;
}

.detail-block {
  display: grid;
  gap: 6px;
  padding-top: 9px;
  border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.14));
}

.detail-block p {
  font-size: 0.82rem;
  line-height: 1.35;
  white-space: pre-wrap;
}

.evidence-list {
  display: grid;
  gap: 7px;
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

.evidence-item span {
  color: var(--tg-theme-hint-color, #6b7b88);
}

.evidence-item p {
  font-size: 0.78rem;
  line-height: 1.35;
  overflow-wrap: anywhere;
}

pre {
  max-height: 260px;
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

@media (max-width: 820px) {
  .content-grid,
  .learning-grid {
    grid-template-columns: minmax(0, 1fr);
  }

  .detail-panel {
    position: static;
  }
}

@media (max-width: 560px) {
  .summary-grid,
  .learning-funnel {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .meta-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
</style>
