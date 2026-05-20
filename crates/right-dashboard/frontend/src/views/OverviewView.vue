<script setup lang="ts">
import MetricCard from '../components/MetricCard.vue'
import StatusPill from '../components/StatusPill.vue'
import { money, shortDate } from '../format'
import type { DashboardOverviewResponse, OverviewResponse } from '../types'

defineProps<{
  overview: DashboardOverviewResponse | null
  activity: OverviewResponse | null
}>()
</script>

<template>
  <section class="metric-grid">
    <MetricCard label="Active" :value="overview?.active_runs ?? 0" tone="active" />
    <MetricCard label="Failures" :value="overview?.recent_failures ?? 0" :tone="(overview?.recent_failures ?? 0) > 0 ? 'bad' : 'ok'" />
    <MetricCard label="Today" :value="money(overview?.today_cost_usd)" />
    <MetricCard label="Candidates" :value="overview?.learning_candidates_24h ?? 0" />
    <MetricCard label="Jobs" :value="activity?.summary.cron_count ?? 0" />
    <MetricCard label="Running cron" :value="activity?.summary.active_cron_count ?? 0" tone="active" />
  </section>

  <section class="two-column">
    <section class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Doctor</p>
          <h2>{{ overview?.doctor.state ?? 'not_loaded' }}</h2>
        </div>
        <StatusPill :status="overview?.doctor.state ?? 'not_loaded'" />
      </header>
      <dl class="meta-grid compact">
        <div>
          <dt>Pass</dt>
          <dd>{{ overview?.doctor.pass_count ?? 0 }}</dd>
        </div>
        <div>
          <dt>Warn</dt>
          <dd>{{ overview?.doctor.warn_count ?? 0 }}</dd>
        </div>
        <div>
          <dt>Fail</dt>
          <dd>{{ overview?.doctor.fail_count ?? 0 }}</dd>
        </div>
        <div>
          <dt>Snapshot</dt>
          <dd>{{ shortDate(overview?.doctor.generated_at) }}</dd>
        </div>
      </dl>
    </section>

    <section class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Sandbox</p>
          <h2>{{ overview?.sandbox.detail ?? overview?.sandbox.state ?? 'unknown' }}</h2>
        </div>
        <StatusPill :status="overview?.sandbox.state ?? 'unknown'" />
      </header>
      <dl class="meta-grid compact">
        <div>
          <dt>Foreground</dt>
          <dd>{{ activity?.active.foreground.length ?? 0 }}</dd>
        </div>
        <div>
          <dt>Background</dt>
          <dd>{{ activity?.active.background.length ?? 0 }}</dd>
        </div>
        <div>
          <dt>Updated</dt>
          <dd>{{ shortDate(overview?.generated_at) }}</dd>
        </div>
      </dl>
    </section>
  </section>
</template>
