<script setup lang="ts">
import { onMounted, ref } from 'vue'

import { focusGet, focusUpdate } from '../api'
import AsyncState from '../components/AsyncState.vue'

const props = defineProps<{
  chatId: number
  threadId: number
  token: string
}>()

// Keep in sync with OPERATOR_FOCUS_MAX_CHARS in the dashboard focus route.
const MAX_FOCUS_CHARS = 4000

const loading = ref(true)
const loadError = ref<string | null>(null)
const saveError = ref<string | null>(null)
const saving = ref(false)
const value = ref('')

onMounted(() => {
  void load()
})

async function load(): Promise<void> {
  loading.value = true
  loadError.value = null
  try {
    const response = await focusGet(props.chatId, props.threadId, props.token)
    value.value = response.operator_focus ?? ''
    saveError.value = null
  } catch (err) {
    loadError.value = err instanceof Error ? err.message : 'Conversation focus unavailable'
  } finally {
    loading.value = false
  }
}

async function save(): Promise<void> {
  if (saving.value) {
    return
  }

  saving.value = true
  saveError.value = null
  try {
    const submitted = value.value
    const response = await focusUpdate(props.chatId, props.threadId, props.token, submitted)
    if (value.value === submitted) {
      value.value = response.operator_focus ?? ''
    }
  } catch (err) {
    saveError.value = err instanceof Error ? err.message : 'Failed to save conversation focus'
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <section class="focus-view panel">
    <h1>Conversation focus</h1>
    <p class="muted-line">Standing context for this conversation, appended to the agent's prompt every turn.</p>
    <AsyncState :loading="loading" :error="loadError" :empty="false">
      <textarea
        v-model="value"
        class="focus-textarea"
        aria-label="Conversation focus"
        rows="10"
        :maxlength="MAX_FOCUS_CHARS"
        placeholder="What should the agent keep in mind in this conversation?"
        @input="saveError = null"
      />
      <p v-if="saveError" class="notice inline" role="alert">{{ saveError }}</p>
      <div class="focus-actions">
        <button class="tool-button" type="button" :disabled="saving" @click="save">
          {{ saving ? 'Saving...' : 'Save' }}
        </button>
      </div>
    </AsyncState>
  </section>
</template>

<style scoped>
.focus-view {
  display: grid;
  gap: 10px;
  width: min(720px, calc(100% - 24px));
  margin: calc(14px + env(safe-area-inset-top)) auto calc(20px + env(safe-area-inset-bottom));
}

.focus-textarea {
  width: 100%;
  min-width: 0;
  min-height: 180px;
  padding: 8px 10px;
  resize: vertical;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  color: var(--tg-theme-text-color, #17212b);
  font: inherit;
  line-height: 1.45;
}

.focus-actions {
  display: flex;
  justify-content: flex-end;
  margin-top: 8px;
}

.tool-button:disabled {
  cursor: not-allowed;
  opacity: 0.58;
}
</style>
