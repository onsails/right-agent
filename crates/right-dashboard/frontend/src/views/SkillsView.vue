<script setup lang="ts">
import StatusPill from '../components/StatusPill.vue'
import type { SkillDetailResponse, SkillSummary, SkillsResponse } from '../types'

const skillGroups = ['core', 'learned', 'other'] as const
type SkillGroupName = typeof skillGroups[number]

defineProps<{
  skills: SkillsResponse | null
  selectedSkill: SkillDetailResponse | null
  selectedSkillName: string | null
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  selectSkill: [skill: SkillSummary]
}>()

function skillsFor(response: SkillsResponse | null, group: SkillGroupName): SkillSummary[] {
  return response?.groups[group] ?? []
}
</script>

<template>
  <section class="two-column wide-main">
    <section class="list-stack">
      <p v-if="skills?.warning" class="notice inline">{{ skills.warning }}</p>
      <article v-for="group in skillGroups" :key="group" class="panel">
        <header class="panel-head">
          <div>
            <p class="eyebrow">Skills</p>
            <h2>{{ group }}</h2>
          </div>
          <StatusPill :status="skills?.source ?? 'unavailable'" />
        </header>
        <div class="row-list">
          <button
            v-for="skill in skillsFor(skills, group)"
            :key="skill.name"
            type="button"
            class="data-row"
            :class="{ selected: selectedSkillName === skill.name }"
            @click="emit('selectSkill', skill)"
          >
            <span class="row-main">
              <strong>{{ skill.name }}</strong>
              <small>{{ skill.description ?? skill.path }}</small>
            </span>
          </button>
          <p v-if="skillsFor(skills, group).length === 0" class="muted-line">None</p>
        </div>
      </article>
    </section>

    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Skill</p>
          <h2>{{ selectedSkill?.skill.name ?? 'None selected' }}</h2>
        </div>
        <StatusPill v-if="selectedSkill" :status="selectedSkill.skill.group" />
      </header>
      <p v-if="loading" class="muted-line">Loading</p>
      <p v-else-if="error" class="notice inline">{{ error }}</p>
      <p v-else-if="!selectedSkill" class="muted-line">No skill selected</p>
      <template v-if="selectedSkill">
        <p class="muted-line">{{ selectedSkill.skill.path }}</p>
        <pre>{{ selectedSkill.content_preview }}<template v-if="selectedSkill.truncated">
... truncated
</template></pre>
      </template>
    </aside>
  </section>
</template>
