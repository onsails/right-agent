<script setup lang="ts">
import MetricCard from '../../components/MetricCard.vue'
import StatusPill from '../../components/StatusPill.vue'
import { percent, shortDate } from '../../format'
import type { LearningOverviewResponse, LearningReportDetailResponse, LearningReportSummary } from '../../types'

defineProps<{
  learning: LearningOverviewResponse | null
  selectedReport: LearningReportDetailResponse | null
  selectedReportId: number | null
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  selectReport: [report: LearningReportSummary]
}>()
</script>

<template>
  <section class="metric-grid">
    <MetricCard label="Signals" :value="learning?.funnel.signals_accepted_24h ?? 0" />
    <MetricCard label="Reports" :value="learning?.funnel.reports_total_24h ?? 0" tone="active" />
    <MetricCard
      label="Candidates"
      :value="(learning?.funnel.create_candidates_24h ?? 0) + (learning?.funnel.update_candidates_24h ?? 0)"
      tone="ok"
    />
    <MetricCard label="Rate" :value="percent(learning?.quality.candidate_rate)" />
  </section>

  <section class="two-column wide-main">
    <section class="list-stack">
      <article v-if="(learning?.recent_reports.length ?? 0) === 0" class="empty-panel">No reports</article>
      <button
        v-for="report in learning?.recent_reports ?? []"
        :key="report.id"
        type="button"
        class="data-row tall"
        :class="{ selected: selectedReportId === report.id }"
        @click="emit('selectReport', report)"
      >
        <span class="row-main">
          <strong>{{ report.candidate_skill_name ?? `report ${report.id}` }}</strong>
          <span>{{ report.candidate_summary ?? report.trigger_kind }}</span>
          <small>{{ report.confidence }} / {{ shortDate(report.created_at) }}</small>
        </span>
        <span class="row-side">
          <StatusPill :status="report.status" />
        </span>
      </button>
    </section>

    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Report</p>
          <h2>{{ selectedReport?.report.candidate_skill_name ?? 'None selected' }}</h2>
        </div>
        <StatusPill v-if="selectedReport" :status="selectedReport.report.status" />
      </header>

      <p v-if="loading" class="muted-line">Loading</p>
      <p v-else-if="error" class="notice inline">{{ error }}</p>
      <p v-else-if="!selectedReport" class="muted-line">No report selected</p>

      <template v-if="selectedReport">
        <dl class="meta-grid compact">
          <div>
            <dt>Confidence</dt>
            <dd>{{ selectedReport.report.confidence }}</dd>
          </div>
          <div>
            <dt>Trigger</dt>
            <dd>{{ selectedReport.report.trigger_kind }}</dd>
          </div>
          <div>
            <dt>Episode</dt>
            <dd>{{ selectedReport.episode?.id ?? 'none' }}</dd>
          </div>
          <div>
            <dt>Notice</dt>
            <dd>{{ selectedReport.reviewer.user_notice_present ? 'present' : 'none' }}</dd>
          </div>
        </dl>

        <section class="text-block">
          <h3>Summary</h3>
          <p>{{ selectedReport.report.candidate_summary ?? selectedReport.reviewer.status }}</p>
        </section>

        <section v-if="selectedReport.selector" class="text-block">
          <h3>Selector</h3>
          <p>{{ selectedReport.selector.boundary_rationale ?? 'No rationale' }}</p>
        </section>

        <section class="text-block">
          <h3>Evidence</h3>
          <div class="evidence-list">
            <div v-for="snippet in selectedReport.evidence" :key="snippet.ref_id" class="evidence-item">
              <strong>{{ snippet.ref_id }}</strong>
              <span>{{ snippet.available ? (snippet.event_kind || snippet.role || snippet.source) : 'unavailable' }}</span>
              <p>{{ snippet.text || 'Snippet unavailable' }}</p>
            </div>
          </div>
        </section>
      </template>
    </aside>
  </section>
</template>
