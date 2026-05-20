<script setup lang="ts">
import StatusPill from '../../components/StatusPill.vue'
import { shortDate } from '../../format'
import type { LearningEpisodeDetailResponse, LearningEpisodeSummary } from '../../types'

defineProps<{
  episodes: LearningEpisodeSummary[]
  selectedEpisode: LearningEpisodeDetailResponse | null
  selectedEpisodeId: number | null
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  selectEpisode: [episode: LearningEpisodeSummary]
}>()
</script>

<template>
  <section class="two-column wide-main">
    <section class="list-stack">
      <article v-if="episodes.length === 0" class="empty-panel">No episodes</article>
      <button
        v-for="episode in episodes"
        :key="episode.id"
        type="button"
        class="data-row tall"
        :class="{ selected: selectedEpisodeId === episode.id }"
        @click="emit('selectEpisode', episode)"
      >
        <span class="row-main">
          <strong>#{{ episode.id }}</strong>
          <span>{{ episode.kind }}</span>
          <small>{{ episode.seed_trigger_kind }}</small>
        </span>
        <span class="row-side">
          <StatusPill :status="episode.status" />
          <small>{{ shortDate(episode.updated_at) }}</small>
        </span>
      </button>
    </section>

    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Episode</p>
          <h2>{{ selectedEpisode?.episode.id ? `#${selectedEpisode.episode.id}` : 'None selected' }}</h2>
        </div>
        <StatusPill v-if="selectedEpisode" :status="selectedEpisode.episode.status" />
      </header>

      <p v-if="loading" class="muted-line">Loading</p>
      <p v-else-if="error" class="notice inline">{{ error }}</p>
      <p v-else-if="!selectedEpisode" class="muted-line">No episode selected</p>

      <template v-if="selectedEpisode">
        <dl class="meta-grid compact">
          <div>
            <dt>Seed</dt>
            <dd>{{ selectedEpisode.episode.seed_ref }}</dd>
          </div>
          <div>
            <dt>Confidence</dt>
            <dd>{{ selectedEpisode.episode.confidence ?? 'none' }}</dd>
          </div>
          <div>
            <dt>Start</dt>
            <dd>{{ selectedEpisode.episode.start_ref ?? 'none' }}</dd>
          </div>
          <div>
            <dt>End</dt>
            <dd>{{ selectedEpisode.episode.end_ref ?? 'none' }}</dd>
          </div>
        </dl>

        <section v-if="selectedEpisode.selector" class="text-block">
          <h3>Selector</h3>
          <p>{{ selectedEpisode.selector.boundary_rationale ?? 'No rationale' }}</p>
        </section>

        <section class="text-block">
          <h3>Reports</h3>
          <div class="row-list">
            <div v-for="report in selectedEpisode.episode.reports" :key="report.id" class="model-row">
              <span>{{ report.candidate_skill_name ?? report.status }}</span>
              <StatusPill :status="report.status" />
            </div>
          </div>
        </section>
      </template>
    </aside>
  </section>
</template>
