<script setup lang="ts">
import { money } from '../../format'
import type { UsageDailyPoint } from '../../types'

defineProps<{
  point: UsageDailyPoint | null
}>()

const integer = new Intl.NumberFormat()

function count(value: number): string {
  return integer.format(value)
}
</script>

<template>
  <aside class="panel detail-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Breakdown</p>
        <h2>{{ point?.date ?? 'None selected' }}</h2>
      </div>
      <strong>{{ money(point?.total_cost_usd) }}</strong>
    </header>
    <p v-if="!point" class="muted-line">Select a day</p>
    <template v-else>
      <dl class="meta-grid compact">
        <div><dt>Subscription</dt><dd>{{ money(point.subscription_cost_usd) }}</dd></div>
        <div><dt>API</dt><dd>{{ money(point.api_cost_usd) }}</dd></div>
        <div><dt>Turns</dt><dd>{{ point.turns }}</dd></div>
        <div><dt>Calls</dt><dd>{{ point.invocations }}</dd></div>
      </dl>
      <section class="text-block">
        <h3>Counters</h3>
        <div class="row-list">
          <div class="model-row">
            <span>Tokens</span>
            <strong>{{ count(point.input_tokens) }} in / {{ count(point.output_tokens) }} out</strong>
          </div>
          <div class="model-row">
            <span>Cache</span>
            <strong>{{ count(point.cache_creation_tokens) }} create / {{ count(point.cache_read_tokens) }} read</strong>
          </div>
          <div class="model-row">
            <span>Web</span>
            <strong>{{ count(point.web_search_requests) }} search / {{ count(point.web_fetch_requests) }} fetch</strong>
          </div>
        </div>
      </section>
      <section class="text-block">
        <h3>Sources</h3>
        <div class="row-list">
          <div v-for="source in (point.sources ?? [])" :key="source.source" class="model-row">
            <span>{{ source.source }}</span>
            <strong>{{ money(source.cost_usd) }}</strong>
          </div>
          <p v-if="(point.sources ?? []).length === 0" class="muted-line">No source spend</p>
        </div>
      </section>
      <section class="text-block">
        <h3>Models</h3>
        <div class="row-list">
          <div v-for="model in (point.models ?? [])" :key="model.model" class="model-row">
            <span>{{ model.model }}</span>
            <strong>{{ money(model.cost_usd) }}</strong>
          </div>
          <p v-if="(point.models ?? []).length === 0" class="muted-line">No model spend</p>
        </div>
      </section>
    </template>
  </aside>
</template>
