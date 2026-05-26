<script setup lang="ts">
import { computed, ref } from 'vue'
import { secretInputType, secretToggleAriaLabel, secretToggleText } from './secretInputModel'

defineProps<{
  modelValue: string
  placeholder?: string
  disabled?: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()

const revealed = ref(false)
const inputType = computed(() => secretInputType(revealed.value))
const toggleText = computed(() => secretToggleText(revealed.value))
const toggleAriaLabel = computed(() => secretToggleAriaLabel(revealed.value))

function update(event: Event): void {
  const target = event.target as HTMLInputElement
  emit('update:modelValue', target.value)
}
</script>

<template>
  <div class="secret-input">
    <input
      class="text-input"
      :type="inputType"
      :value="modelValue"
      :placeholder="placeholder"
      :disabled="disabled"
      autocomplete="off"
      @input="update"
    >
    <button
      class="secret-toggle"
      type="button"
      :aria-label="toggleAriaLabel"
      :disabled="disabled"
      @click="revealed = !revealed"
    >
      {{ toggleText }}
    </button>
  </div>
</template>

<style scoped>
.secret-input {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 6px;
  min-width: 0;
}

.text-input {
  width: 100%;
  min-width: 0;
  min-height: 32px;
  padding: 5px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
}

.secret-toggle {
  min-width: 48px;
  min-height: 32px;
  padding: 5px 8px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-text-color, #17212b);
  cursor: pointer;
  white-space: nowrap;
}

.secret-toggle:disabled {
  cursor: not-allowed;
  opacity: 0.58;
}
</style>
