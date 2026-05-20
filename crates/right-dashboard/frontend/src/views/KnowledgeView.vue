<script setup lang="ts">
import EpisodesView from './learning/EpisodesView.vue'
import ReportsView from './learning/ReportsView.vue'
import SkillsView from './SkillsView.vue'
import type {
  LearningEpisodeDetailResponse,
  LearningEpisodeSummary,
  LearningEpisodesResponse,
  LearningOverviewResponse,
  LearningReportDetailResponse,
  LearningReportSummary,
  SkillDetailResponse,
  SkillSummary,
  SkillsResponse,
} from '../types'

defineProps<{
  activeSubtab: 'episodes' | 'reports' | 'skills'
  learning: LearningOverviewResponse | null
  episodes: LearningEpisodesResponse | null
  selectedEpisode: LearningEpisodeDetailResponse | null
  selectedEpisodeId: number | null
  selectedReport: LearningReportDetailResponse | null
  selectedReportId: number | null
  selectedSkill: SkillDetailResponse | null
  selectedSkillName: string | null
  skills: SkillsResponse | null
  loadingEpisode: boolean
  loadingReport: boolean
  loadingSkill: boolean
  episodeError: string | null
  reportError: string | null
  skillError: string | null
}>()

const emit = defineEmits<{
  setSubtab: [tab: 'episodes' | 'reports' | 'skills']
  selectEpisode: [episode: LearningEpisodeSummary]
  selectReport: [report: LearningReportSummary]
  selectSkill: [skill: SkillSummary]
}>()
</script>

<template>
  <nav class="subtabs" aria-label="Knowledge views">
    <button type="button" class="tab-button" :class="{ active: activeSubtab === 'episodes' }" @click="emit('setSubtab', 'episodes')">Episodes</button>
    <button type="button" class="tab-button" :class="{ active: activeSubtab === 'reports' }" @click="emit('setSubtab', 'reports')">Reports</button>
    <button type="button" class="tab-button" :class="{ active: activeSubtab === 'skills' }" @click="emit('setSubtab', 'skills')">Skills</button>
  </nav>

  <EpisodesView
    v-if="activeSubtab === 'episodes'"
    :episodes="episodes?.episodes ?? []"
    :selected-episode="selectedEpisode"
    :selected-episode-id="selectedEpisodeId"
    :loading="loadingEpisode"
    :error="episodeError"
    @select-episode="emit('selectEpisode', $event)"
  />
  <ReportsView
    v-else-if="activeSubtab === 'reports'"
    :learning="learning"
    :selected-report="selectedReport"
    :selected-report-id="selectedReportId"
    :loading="loadingReport"
    :error="reportError"
    @select-report="emit('selectReport', $event)"
  />
  <SkillsView
    v-else
    :skills="skills"
    :selected-skill="selectedSkill"
    :selected-skill-name="selectedSkillName"
    :loading="loadingSkill"
    :error="skillError"
    @select-skill="emit('selectSkill', $event)"
  />
</template>
