<script setup lang="ts">
import { computed } from 'vue'
import { statusTone } from '../format'

const props = defineProps<{
  status: string
  label?: string
}>()

const tone = computed(() => statusTone(props.status))
</script>

<template>
  <span class="status-pill" :class="tone">
    {{ label ?? status }}
  </span>
</template>
