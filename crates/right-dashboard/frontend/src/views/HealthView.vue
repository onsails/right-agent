<script setup lang="ts">
import StatusPill from '../components/StatusPill.vue'
import { bytes, shortDate } from '../format'
import type { DoctorCheckResponse, DoctorResponse, SandboxStatsResponse } from '../types'

defineProps<{
  doctor: DoctorResponse | null
  sandbox: SandboxStatsResponse | null
  loadingDoctor: boolean
  loadingSandbox: boolean
  doctorError: string | null
  sandboxError: string | null
}>()

const emit = defineEmits<{
  refreshDoctor: []
  refreshSandbox: []
}>()

function checkRows(doctor: DoctorResponse | null): DoctorCheckResponse[] {
  return [...(doctor?.fail ?? []), ...(doctor?.warn ?? []), ...(doctor?.pass ?? [])]
}
</script>

<template>
  <section class="two-column">
    <section class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Doctor</p>
          <h2>{{ doctor ? `${doctor.pass_count}/${doctor.pass_count + doctor.warn_count + doctor.fail_count}` : 'not loaded' }}</h2>
        </div>
        <button type="button" class="tool-button" @click="emit('refreshDoctor')">
          {{ loadingDoctor ? 'Running' : 'Refresh' }}
        </button>
      </header>
      <p v-if="doctorError" class="notice inline">{{ doctorError }}</p>
      <dl v-if="doctor" class="meta-grid compact">
        <div>
          <dt>Pass</dt>
          <dd>{{ doctor.pass_count }}</dd>
        </div>
        <div>
          <dt>Warn</dt>
          <dd>{{ doctor.warn_count }}</dd>
        </div>
        <div>
          <dt>Fail</dt>
          <dd>{{ doctor.fail_count }}</dd>
        </div>
        <div>
          <dt>Updated</dt>
          <dd>{{ shortDate(doctor.generated_at) }}</dd>
        </div>
      </dl>
      <div class="row-list">
        <div v-for="check in checkRows(doctor)" :key="`${check.status}:${check.name}`" class="data-row static">
          <span class="row-main">
            <strong>{{ check.name }}</strong>
            <small>{{ check.detail }}</small>
          </span>
          <StatusPill :status="check.status" />
        </div>
      </div>
    </section>

    <section class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Sandbox</p>
          <h2>{{ sandbox?.source ?? 'not loaded' }}</h2>
        </div>
        <button type="button" class="tool-button" @click="emit('refreshSandbox')">
          {{ loadingSandbox ? 'Reading' : 'Refresh' }}
        </button>
      </header>
      <p v-if="sandboxError" class="notice inline">{{ sandboxError }}</p>
      <p v-if="sandbox?.warning" class="notice inline">{{ sandbox.warning }}</p>
      <dl class="meta-grid compact">
        <div>
          <dt>Free</dt>
          <dd>{{ bytes(sandbox?.disk?.available_bytes) }}</dd>
        </div>
        <div>
          <dt>Used</dt>
          <dd>{{ sandbox?.disk ? `${sandbox.disk.used_percent.toFixed(0)}%` : 'none' }}</dd>
        </div>
        <div>
          <dt>RAM</dt>
          <dd>{{ bytes(sandbox?.memory?.used_bytes) }}</dd>
        </div>
        <div>
          <dt>Load</dt>
          <dd>{{ sandbox?.memory?.load_average_1m ?? 'none' }}</dd>
        </div>
      </dl>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>PID</th>
              <th>CPU</th>
              <th>RAM</th>
              <th>RSS</th>
              <th>Command</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="process in sandbox?.processes ?? []" :key="process.pid">
              <td>{{ process.pid }}</td>
              <td>{{ process.cpu_percent.toFixed(1) }}</td>
              <td>{{ process.memory_percent.toFixed(1) }}</td>
              <td>{{ bytes(process.rss_bytes) }}</td>
              <td>{{ process.command }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
  </section>
</template>
