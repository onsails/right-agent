<script setup lang="ts">
import type { ProviderProfileView } from '../types'

defineProps<{ types: ProviderProfileView[] }>()
const emit = defineEmits<{ (e: 'select', t: ProviderProfileView): void }>()

function select(t: ProviderProfileView): void {
  emit('select', t)
}
</script>

<template>
  <div class="type-grid">
    <p class="muted-line">Choose a provider type:</p>
    <article
      v-for="t in types"
      :key="t.type"
      class="type-card"
      @click="select(t)"
    >
      <strong>{{ t.display_name }}</strong>
      <small>{{ t.category }}</small>
      <small>{{ t.env_var }}</small>
    </article>
    <p v-if="types.length === 0" class="muted-line">No provider types available</p>
  </div>
</template>

<style scoped>
.type-grid {
  display: grid;
  gap: 8px;
  grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
}

.type-card {
  display: grid;
  gap: 2px;
  padding: 8px 10px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-bg-color, #f4f6f8);
  cursor: pointer;
}

.type-card:hover {
  border-color: var(--tg-theme-button_color, var(--jewel-teal));
}

.type-card strong {
  font-size: 0.84rem;
}

.type-card small {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.72rem;
}
</style>
