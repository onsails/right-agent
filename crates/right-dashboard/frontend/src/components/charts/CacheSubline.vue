<script setup lang="ts">
import { compactCount, percent } from '../../format'
import { cacheHitRate, type CacheTokens } from './usageCache'

defineProps<{
  tokens: CacheTokens
}>()
</script>

<template>
  <p v-if="tokens.cache_read_tokens > 0" class="muted-line cache-subline">
    {{ compactCount(tokens.cache_creation_tokens) }} created ·
    {{ compactCount(tokens.cache_read_tokens) }} read ·
    {{ percent(cacheHitRate(tokens)) }} hit
  </p>
</template>

<style scoped>
.cache-subline {
  font-size: 0.72rem;
  margin: 2px 0 0;
  padding-left: 8px;
}
</style>
