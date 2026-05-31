<script setup lang="ts">
import { computed, ref } from 'vue'

import { DashboardApiError, skillDetail, skillsOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import SkillsView from './SkillsView.vue'
import type { SkillDetailResponse, SkillSummary, SkillsResponse } from '../types'

const { data: skills, refresh } = useLiveResource(skillsOverview, { key: 'skills' })

const pinnedPatches = ref(new Map<string, boolean>())

const patchedSkills = computed((): SkillsResponse | null => {
  const base = skills.value
  if (base === null) return null
  if (pinnedPatches.value.size === 0) return base
  const patchGroup = (group: SkillSummary[]): SkillSummary[] =>
    group.map((skill) => {
      const pinned = pinnedPatches.value.get(skill.name)
      return pinned === undefined ? skill : { ...skill, pinned }
    })
  return {
    ...base,
    groups: {
      core: patchGroup(base.groups.core),
      learned: patchGroup(base.groups.learned),
      other: patchGroup(base.groups.other),
    },
  }
})

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
  pinnedPatches.value = new Map(pinnedPatches.value).set(skillName, pinned)
}
</script>

<template>
  <SkillsView
    :skills="patchedSkills"
    :selected-skill="selectedSkill"
    :selected-skill-name="selectedSkillName"
    :loading="loadingSkill"
    :error="skillError"
    @select-skill="selectSkill"
    @skill-pinned="applySkillPinned"
  />
</template>
