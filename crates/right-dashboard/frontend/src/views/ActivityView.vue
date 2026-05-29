<script setup lang="ts">
import AsyncState from '../components/AsyncState.vue'
import MetricCard from '../components/MetricCard.vue'
import StatusPill from '../components/StatusPill.vue'
import { deliveryLabel, deliveryTone, money, notifyText, shortDate, shortId, statusTone } from '../format'
import type { CronCard, OverviewResponse, RunDetailResponse, RunSummary } from '../types'

defineProps<{
  overview: OverviewResponse | null
  selectedRun: RunDetailResponse | null
  selectedRunId: string | null
  loadingDetail: boolean
  detailError: string | null
}>()

const emit = defineEmits<{
  selectRun: [run: RunSummary]
}>()

function cronStatus(cron: CronCard): string {
  const active = cron.recent_runs.find((run) => run.status === 'queued' || run.status === 'running')
  return active?.status ?? cron.last_run?.status ?? 'idle'
}
</script>

<template>
  <section class="metric-grid">
    <MetricCard label="Jobs" :value="overview?.summary.cron_count ?? 0" />
    <MetricCard label="Running" :value="overview?.summary.active_cron_count ?? 0" tone="active" />
    <MetricCard label="Failures" :value="overview?.summary.failed_recent_cron_count ?? 0" tone="bad" />
    <MetricCard label="Today" :value="money(overview?.summary.today_cost_usd)" />
  </section>

  <section class="two-column wide-main">
    <section class="list-stack">
      <article v-if="(overview?.crons.length ?? 0) === 0" class="empty-panel">No cron jobs</article>

      <article v-for="cron in overview?.crons ?? []" :key="cron.job_name" class="panel">
        <header class="panel-head">
          <div>
            <p class="eyebrow">{{ cron.recurring ? 'Recurring' : 'One shot' }}</p>
            <h2>{{ cron.job_name }}</h2>
            <p class="muted-line">{{ cron.schedule }}</p>
          </div>
          <StatusPill :status="cronStatus(cron)" />
        </header>

        <dl class="meta-grid">
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
          <div>
            <dt>Recent</dt>
            <dd>{{ cron.recent_runs.length }}</dd>
          </div>
        </dl>

        <div class="row-list">
          <template v-for="run in cron.recent_runs" :key="run.id">
            <button
              class="data-row"
              :class="{ selected: selectedRunId === run.id }"
              type="button"
              @click="emit('selectRun', run)"
            >
              <span class="row-main">
                <span class="status-dot" :class="statusTone(run.status)"></span>
                <strong>{{ run.status }}</strong>
                <small>{{ shortId(run.id) }}</small>
                <span class="run-delivery-badge" :class="deliveryTone(run)">{{ deliveryLabel(run) }}</span>
                <small v-if="run.run_note" class="run-note-preview">{{ run.run_note }}</small>
              </span>
              <span class="row-side">
                <strong>{{ money(run.cost_usd) }}</strong>
                <small>{{ shortDate(run.started_at) }}</small>
              </span>
            </button>

            <section v-if="selectedRunId === run.id" class="run-inline-detail">
              <AsyncState
                :loading="loadingDetail"
                :error="detailError"
                :empty="!selectedRun || selectedRun.run.id !== run.id"
                empty-text="No run detail"
              >
                <dl class="meta-grid compact">
                  <div>
                    <dt>Kind</dt>
                    <dd>{{ selectedRun!.run.kind }}</dd>
                  </div>
                  <div>
                    <dt>Delivery</dt>
                    <dd>{{ deliveryLabel(selectedRun!.run) }}</dd>
                  </div>
                  <div>
                    <dt>Exit</dt>
                    <dd>{{ selectedRun!.run.exit_code ?? 'none' }}</dd>
                  </div>
                  <div>
                    <dt>Cost</dt>
                    <dd>{{ money(selectedRun!.run.cost_usd) }}</dd>
                  </div>
                  <div>
                    <dt>Started</dt>
                    <dd>{{ shortDate(selectedRun!.run.started_at) }}</dd>
                  </div>
                  <div>
                    <dt>Finished</dt>
                    <dd>{{ shortDate(selectedRun!.run.finished_at) }}</dd>
                  </div>
                </dl>

                <section class="text-block">
                  <h3>Run note</h3>
                  <p>{{ selectedRun!.run_note || 'No run note' }}</p>
                </section>
                <section v-if="notifyText(selectedRun!.delivery)" class="text-block">
                  <h3>Delivery</h3>
                  <pre>{{ notifyText(selectedRun!.delivery) }}</pre>
                </section>
                <section v-if="selectedRun!.delivery_error" class="text-block">
                  <h3>Delivery error</h3>
                  <p>{{ selectedRun!.delivery_error }}</p>
                </section>
                <section v-if="selectedRun!.error_message" class="text-block">
                  <h3>Error</h3>
                  <p>{{ selectedRun!.error_message }}</p>
                </section>
                <section class="text-block">
                  <h3>Log</h3>
                  <p v-if="!selectedRun!.log.available" class="muted-line">Log unavailable</p>
                  <pre v-else>{{ selectedRun!.log.lines.join('\n') }}<template v-if="selectedRun!.log.truncated">
... truncated
</template></pre>
                </section>
              </AsyncState>
            </section>
          </template>
          <p v-if="cron.recent_runs.length === 0" class="muted-line">No recent runs</p>
        </div>
      </article>
    </section>

    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Run</p>
          <h2>{{ selectedRun?.run.id ? shortId(selectedRun.run.id) : 'None selected' }}</h2>
        </div>
        <StatusPill v-if="selectedRun" :status="selectedRun.run.status" />
      </header>

      <!-- AsyncState renders its default slot only when :empty is false, i.e. only when
           selectedRun is non-null, so the selectedRun! assertions in the slot below are safe. -->
      <AsyncState
        :loading="loadingDetail"
        :error="detailError"
        :empty="!selectedRun"
        empty-text="No run selected"
      >
        <dl class="meta-grid compact">
          <div>
            <dt>Kind</dt>
            <dd>{{ selectedRun!.run.kind }}</dd>
          </div>
          <div>
            <dt>Delivery</dt>
            <dd>{{ deliveryLabel(selectedRun!.run) }}</dd>
          </div>
          <div>
            <dt>Exit</dt>
            <dd>{{ selectedRun!.run.exit_code ?? 'none' }}</dd>
          </div>
          <div>
            <dt>Cost</dt>
            <dd>{{ money(selectedRun!.run.cost_usd) }}</dd>
          </div>
          <div>
            <dt>Started</dt>
            <dd>{{ shortDate(selectedRun!.run.started_at) }}</dd>
          </div>
          <div>
            <dt>Finished</dt>
            <dd>{{ shortDate(selectedRun!.run.finished_at) }}</dd>
          </div>
        </dl>

        <section class="text-block">
          <h3>Run note</h3>
          <p>{{ selectedRun!.run_note || 'No run note' }}</p>
        </section>
        <section v-if="notifyText(selectedRun!.delivery)" class="text-block">
          <h3>Delivery</h3>
          <pre>{{ notifyText(selectedRun!.delivery) }}</pre>
        </section>
        <section v-if="selectedRun!.delivery_error" class="text-block">
          <h3>Delivery error</h3>
          <p>{{ selectedRun!.delivery_error }}</p>
        </section>
        <section v-if="selectedRun!.error_message" class="text-block">
          <h3>Error</h3>
          <p>{{ selectedRun!.error_message }}</p>
        </section>
        <section class="text-block">
          <h3>Log</h3>
          <p v-if="!selectedRun!.log.available" class="muted-line">Log unavailable</p>
          <pre v-else>{{ selectedRun!.log.lines.join('\n') }}<template v-if="selectedRun!.log.truncated">
... truncated
</template></pre>
        </section>
      </AsyncState>
    </aside>
  </section>
</template>
