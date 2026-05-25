<script setup lang="ts">
import ReportsView from './learning/ReportsView.vue'
import SkillsView from './SkillsView.vue'
import type {
  LearningOverviewResponse,
  SkillDetailResponse,
  SkillSummary,
  SkillsResponse,
} from '../types'

defineProps<{
  activeSubtab: 'learning' | 'skills'
  learning: LearningOverviewResponse | null
  selectedSkill: SkillDetailResponse | null
  selectedSkillName: string | null
  skills: SkillsResponse | null
  loadingSkill: boolean
  skillError: string | null
}>()

const emit = defineEmits<{
  setSubtab: [tab: 'learning' | 'skills']
  selectSkill: [skill: SkillSummary]
  skillPinned: [payload: { skillName: string, pinned: boolean }]
}>()
</script>

<template>
  <nav class="subtabs" aria-label="Knowledge views">
    <button type="button" class="tab-button" :class="{ active: activeSubtab === 'learning' }" @click="emit('setSubtab', 'learning')">Learning</button>
    <button type="button" class="tab-button" :class="{ active: activeSubtab === 'skills' }" @click="emit('setSubtab', 'skills')">Skills</button>
  </nav>

  <ReportsView v-if="activeSubtab === 'learning'" :learning="learning" />
  <SkillsView
    v-else
    :skills="skills"
    :selected-skill="selectedSkill"
    :selected-skill-name="selectedSkillName"
    :loading="loadingSkill"
    :error="skillError"
    @select-skill="emit('selectSkill', $event)"
    @skill-pinned="emit('skillPinned', $event)"
  />
</template>
