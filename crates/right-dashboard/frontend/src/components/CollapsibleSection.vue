<script setup lang="ts">
import { ref } from 'vue'

const props = withDefaults(defineProps<{
  title: string
  count: number
  defaultOpen?: boolean
}>(), { defaultOpen: false })

const open = ref(props.defaultOpen)
</script>

<template>
  <article class="panel collapsible">
    <button
      type="button"
      class="panel-head collapsible-head"
      :aria-expanded="open"
      @click="open = !open"
    >
      <span class="collapsible-title">
        <span class="chevron" :class="{ open }" aria-hidden="true">›</span>
        <strong>{{ title }}</strong>
        <span class="count-badge">{{ count }}</span>
      </span>
    </button>
    <div v-if="open" class="collapsible-body">
      <slot />
    </div>
  </article>
</template>

<style scoped>
.collapsible-head {
  display: flex;
  width: 100%;
  align-items: center;
  justify-content: space-between;
  background: none;
  border: 0;
  cursor: pointer;
  text-align: left;
  color: inherit;
  margin-bottom: 0;
}
.collapsible-title {
  display: flex;
  align-items: center;
  gap: 8px;
}
.count-badge {
  font-size: 0.78em;
  padding: 1px 8px;
  border-radius: 999px;
  background: var(--tg-theme-secondary-bg-color, rgba(127, 127, 127, 0.18));
}
.chevron {
  transition: transform 0.15s ease;
}
.chevron.open {
  transform: rotate(90deg);
}
</style>
