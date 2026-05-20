<script setup lang="ts">
import { money } from '../format'
import type { UsageOverviewResponse, UsageWindow } from '../types'

defineProps<{
  usage: UsageOverviewResponse | null
}>()

function windowRows(window: UsageWindow | null | undefined) {
  return window?.sources ?? []
}
</script>

<template>
  <section class="list-stack">
    <article v-for="window in usage?.windows ?? []" :key="window.key" class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">{{ window.key }}</p>
          <h2>{{ window.label }}</h2>
        </div>
        <strong>{{ money(window.total_cost_usd) }}</strong>
      </header>

      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Source</th>
              <th>Cost</th>
              <th>Turns</th>
              <th>Calls</th>
              <th>Input</th>
              <th>Output</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="source in windowRows(window)" :key="source.source">
              <td>{{ source.source }}</td>
              <td>{{ money(source.cost_usd) }}</td>
              <td>{{ source.turns }}</td>
              <td>{{ source.invocations }}</td>
              <td>{{ source.input_tokens }}</td>
              <td>{{ source.output_tokens }}</td>
            </tr>
          </tbody>
        </table>
      </div>

      <div class="model-grid">
        <div v-for="model in window.per_model" :key="model.model" class="model-row">
          <span>{{ model.model }}</span>
          <strong>{{ money(model.cost_usd) }}</strong>
        </div>
      </div>
    </article>

    <article v-if="!usage" class="empty-panel">No usage snapshot</article>
  </section>
</template>
