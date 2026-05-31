<script setup lang="ts">
import { ref } from 'vue'

import { DashboardApiError, skillDetail, skillsOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import SkillsView from './SkillsView.vue'
import type { SkillDetailResponse, SkillSummary } from '../types'

const { data: skills, refresh } = useLiveResource(skillsOverview, { key: 'skills' })

const selectedSkill = ref<SkillDetailResponse | null>(null)
const selectedSkillName = ref<string | null>(null)
const loadingSkill = ref(false)
const skillError = ref<string | null>(null)

async function selectSkill(skill: SkillSummary): Promise<void> {
  selectedSkillName.value = skill.name
  selectedSkill.value = null
  loadingSkill.value = true
  skillError.value = null
  try {
    const detail = await skillDetail(skill.name)
    if (selectedSkillName.value === skill.name) {
      selectedSkill.value = detail
    }
  } catch (err) {
    if (err instanceof DashboardApiError && err.isLocked) {
      void refresh()
    }
    if (selectedSkillName.value === skill.name) {
      skillError.value = err instanceof Error ? err.message : 'Skill unavailable'
    }
  } finally {
    if (selectedSkillName.value === skill.name) {
      loadingSkill.value = false
    }
  }
}

function applySkillPinned({ skillName, pinned }: { skillName: string, pinned: boolean }): void {
  if (selectedSkill.value && selectedSkill.value.skill.name === skillName) {
    selectedSkill.value = {
      ...selectedSkill.value,
      skill: { ...selectedSkill.value.skill, pinned },
    }
  }
  const current = skills.value
  if (current === null) {
    return
  }
  const updateGroup = (group: SkillSummary[]): SkillSummary[] =>
    group.map((skill) => (skill.name === skillName ? { ...skill, pinned } : skill))
  skills.value = {
    ...current,
    groups: {
      core: updateGroup(current.groups.core),
      learned: updateGroup(current.groups.learned),
      other: updateGroup(current.groups.other),
    },
  }
}
</script>

<template>
  <SkillsView
    :skills="skills"
    :selected-skill="selectedSkill"
    :selected-skill-name="selectedSkillName"
    :loading="loadingSkill"
    :error="skillError"
    @select-skill="selectSkill"
    @skill-pinned="applySkillPinned"
  />
</template>
