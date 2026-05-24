<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { setSkillPinned } from '../api'
import StatusPill from '../components/StatusPill.vue'
import { shortDate } from '../format'
import type { SkillDetailResponse, SkillSummary, SkillsResponse } from '../types'

const skillGroups = ['core', 'learned', 'other'] as const
type SkillGroupName = typeof skillGroups[number]

const props = defineProps<{
  skills: SkillsResponse | null
  selectedSkill: SkillDetailResponse | null
  selectedSkillName: string | null
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  selectSkill: [skill: SkillSummary]
}>()

const pinningSkillName = ref<string | null>(null)
const pinError = ref<string | null>(null)

const selectedCanPin = computed(() => props.selectedSkill !== null && canPinSkill(props.selectedSkill.skill))
const selectedIsPinning = computed(() => pinningSkillName.value === props.selectedSkill?.skill.name)

watch(() => props.selectedSkillName, () => {
  pinError.value = null
})

function skillsFor(response: SkillsResponse | null, group: SkillGroupName): SkillSummary[] {
  return response?.groups[group] ?? []
}

function hasLifecycleRow(skill: SkillSummary): boolean {
  return skill.state !== null && skill.created_by !== null
}

function canPinSkill(skill: SkillSummary): boolean {
  return skill.group === 'learned'
    && skill.name.startsWith('rightx-')
    && hasLifecycleRow(skill)
    && (skill.created_by === 'probe_writer' || skill.created_by === 'curator')
}

function pinLabel(skill: SkillSummary): string {
  return skill.pinned ? 'Pinned' : 'Unpinned'
}

function lifecycleLabel(value: string | null): string {
  return value?.replace(/_/g, ' ') ?? 'none'
}

async function togglePinned(): Promise<void> {
  const skill = props.selectedSkill?.skill
  if (!skill || !canPinSkill(skill) || pinningSkillName.value !== null) {
    return
  }

  const targetPinned = !skill.pinned
  pinningSkillName.value = skill.name
  pinError.value = null
  try {
    const response = await setSkillPinned(skill.name, targetPinned)
    applyPinnedState(response.skill_name, response.pinned)
  } catch (error) {
    pinError.value = error instanceof Error ? error.message : 'Skill unavailable'
  } finally {
    if (pinningSkillName.value === skill.name) {
      pinningSkillName.value = null
    }
  }
}

function applyPinnedState(skillName: string, pinned: boolean): void {
  if (props.selectedSkill?.skill.name === skillName) {
    props.selectedSkill.skill.pinned = pinned
  }

  if (props.skills === null) {
    return
  }

  for (const group of skillGroups) {
    const skill = props.skills.groups[group].find((candidate) => candidate.name === skillName)
    if (skill) {
      skill.pinned = pinned
    }
  }
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
            <span v-if="hasLifecycleRow(skill)" class="row-side">
              <span>{{ pinLabel(skill) }}</span>
              <small>{{ lifecycleLabel(skill.state) }}</small>
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
        <div v-if="selectedSkill" class="detail-statuses">
          <StatusPill :status="selectedSkill.skill.group" />
          <StatusPill :status="selectedSkill.skill.pinned ? 'active' : 'muted'" :label="pinLabel(selectedSkill.skill)" />
        </div>
      </header>
      <p v-if="loading" class="muted-line">Loading</p>
      <p v-else-if="error || pinError" class="notice inline">{{ error ?? pinError }}</p>
      <p v-else-if="!selectedSkill" class="muted-line">No skill selected</p>
      <template v-if="selectedSkill">
        <p class="muted-line">{{ selectedSkill.skill.path }}</p>
        <div v-if="selectedCanPin" class="detail-toolbar">
          <button
            type="button"
            class="tool-button pin-toggle"
            :disabled="selectedIsPinning"
            @click="togglePinned"
          >
            {{ selectedIsPinning ? 'Saving' : (selectedSkill.skill.pinned ? 'Unpin' : 'Pin') }}
          </button>
        </div>
        <dl class="meta-grid compact">
          <div>
            <dt>State</dt>
            <dd>{{ lifecycleLabel(selectedSkill.skill.state) }}</dd>
          </div>
          <div>
            <dt>Pin</dt>
            <dd>{{ pinLabel(selectedSkill.skill) }}</dd>
          </div>
          <div>
            <dt>Created by</dt>
            <dd>{{ lifecycleLabel(selectedSkill.skill.created_by) }}</dd>
          </div>
          <div>
            <dt>Uses</dt>
            <dd>{{ selectedSkill.skill.use_count }}</dd>
          </div>
          <div>
            <dt>Patches</dt>
            <dd>{{ selectedSkill.skill.patch_count }}</dd>
          </div>
          <div>
            <dt>Created</dt>
            <dd>{{ shortDate(selectedSkill.skill.created_at) }}</dd>
          </div>
          <div>
            <dt>Last used</dt>
            <dd>{{ shortDate(selectedSkill.skill.last_used_at) }}</dd>
          </div>
          <div>
            <dt>Last patched</dt>
            <dd>{{ shortDate(selectedSkill.skill.last_patched_at) }}</dd>
          </div>
        </dl>
        <pre>{{ selectedSkill.content_preview }}<template v-if="selectedSkill.truncated">
... truncated
</template></pre>
      </template>
    </aside>
  </section>
</template>

<style scoped>
.detail-statuses,
.detail-toolbar {
  display: flex;
  min-width: 0;
  flex-wrap: wrap;
  gap: 6px;
  justify-content: flex-end;
}

.detail-toolbar {
  justify-content: flex-start;
}

.pin-toggle {
  min-width: 72px;
}

.tool-button:disabled {
  cursor: wait;
  opacity: 0.58;
}
</style>
