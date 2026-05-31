<script setup lang="ts">
import { computed } from 'vue'
import { compactCount, percent } from '../../format'
import { cacheHitRate } from './usageCache'
import { hitSegments, type TokenCounts } from './tokenBar'

const props = defineProps<{
  tokens: TokenCounts
  compact?: boolean
}>()

const segments = computed(() => hitSegments(props.tokens))
const hit = computed(() => percent(cacheHitRate(props.tokens)))
</script>

<template>
  <div class="token-line" :class="{ compact }">
    <div class="token-nums">
      <span class="tok tok-input">{{ compactCount(tokens.input_tokens) }}</span>
      <span class="tok tok-output">{{ compactCount(tokens.output_tokens) }}</span>
      <span class="tok tok-create">{{ compactCount(tokens.cache_creation_tokens) }}</span>
      <span class="tok tok-read">{{ compactCount(tokens.cache_read_tokens) }}</span>
    </div>
    <div v-if="segments" class="token-hit">
      <span class="hit-bar" role="img" :aria-label="`cache hit ${hit}`">
        <span class="seg seg-miss" :style="{ width: `${segments.miss * 100}%` }" />
        <span class="seg seg-create" :style="{ width: `${segments.create * 100}%` }" />
        <span class="seg seg-read" :style="{ width: `${segments.read * 100}%` }" />
      </span>
      <span class="hit-pct">{{ hit }}</span>
    </div>
  </div>
</template>

<style scoped>
.token-line {
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.token-line.compact {
  flex-direction: row;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.token-nums {
  display: flex;
  gap: 8px;
  font-size: 0.72rem;
}
.tok {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  color: var(--tg-theme-text-color, #17212b);
}
.tok::before {
  content: '';
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--dot);
}
.tok-input { --dot: var(--token-input); }
.tok-output { --dot: var(--token-output); }
.tok-create { --dot: var(--token-create); }
.tok-read { --dot: var(--token-read); }
.token-hit {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 0.72rem;
}
.hit-bar {
  display: inline-flex;
  height: 8px;
  min-width: 120px;
  flex: 1;
  border-radius: 4px;
  overflow: hidden;
  background: var(--tg-theme-secondary-bg-color, #e8edf1);
}
.compact .hit-bar {
  min-width: 80px;
  max-width: 160px;
  flex: 0 1 140px;
}
.seg {
  display: block;
  height: 100%;
}
.seg-miss { background: var(--token-input); }
.seg-create { background: var(--token-create); }
.seg-read { background: var(--token-read); }
.hit-pct {
  color: var(--tg-theme-hint-color, #6b7b88);
}
</style>
