<script setup lang="ts">
import { ref } from 'vue'

import ReportsContainer from './learning/ReportsContainer.vue'
import SkillsContainer from './SkillsContainer.vue'

const activeSubtab = ref<'learning' | 'skills'>('learning')
</script>

<template>
  <nav class="subtabs" aria-label="Knowledge views">
    <button type="button" class="tab-button" :class="{ active: activeSubtab === 'learning' }" @click="activeSubtab = 'learning'">Learning</button>
    <button type="button" class="tab-button" :class="{ active: activeSubtab === 'skills' }" @click="activeSubtab = 'skills'">Skills</button>
  </nav>

  <ReportsContainer v-if="activeSubtab === 'learning'" />
  <SkillsContainer v-else />
</template>
