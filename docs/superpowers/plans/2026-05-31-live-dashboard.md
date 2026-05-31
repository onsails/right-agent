# Live Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every dashboard tab refresh on its own (no manual reload) via one reusable Vue polling primitive, and make "live" the default for any future tab.

**Architecture:** A `useLiveResource` composable owns all polling (interval, visibility-pause, race-guard, stale-vs-error, connection reporting). A thin smart **container** per tab calls it and passes `data`/`loading`/`error` down to the existing **dumb** view as the exact props it already takes. Dumb views and their SSR tests are unchanged. `App.vue` shrinks to bootstrap + tab routing + the connection pill (fed by a shared `liveStatus` store).

**Tech Stack:** Vue 3.5 (`<script setup>`, Composition API), TypeScript, Vitest (SSR `renderToString` for views, direct unit tests for pure logic), Vite. Frontend dir: `crates/right-dashboard/frontend`.

**Spec:** `docs/superpowers/specs/2026-05-31-live-dashboard-design.md`

**Working location:** This repo's main checkout is shared by concurrent sessions. Commit each task atomically (`git add <exact files> && git commit` in one step) and verify you are on the intended branch before committing. Never `git add -A`.

---

## Commands reference

- Run one test file: `pnpm -C crates/right-dashboard/frontend exec vitest run src/<path>.test.ts`
- Typecheck: `pnpm -C crates/right-dashboard/frontend typecheck`
- Build (vue-tsc + vite): `pnpm -C crates/right-dashboard/frontend build`
- Final (mandatory): `devenv shell -- cargo test --workspace` (drives `right-dashboard/build.rs` → `vite build`)

## File structure

**New (`crates/right-dashboard/frontend/src/`):**
- `composables/liveStatus.ts` — `ConnectionState`, pure `reduceConnectionState`, pure `classifyOutcome`, reactive registry (`registerLiveResource`, `globalConnectionState`, `globalLastUpdatedAt`).
- `composables/liveStatus.test.ts` — unit tests for the two pure functions.
- `composables/liveConfig.ts` — `provideLiveConfig` / `useLiveConfig` (interval via provide/inject), `DEFAULT_INTERVAL_MS`.
- `composables/useLiveResource.ts` — the polling composable + pure `shouldTick`.
- `composables/useLiveResource.test.ts` — unit tests for `shouldTick`.
- `views/activitySelection.ts` — pure `activityContainsRun`.
- `views/activitySelection.test.ts` — unit tests.
- `views/OverviewContainer.vue`, `views/ActivityContainer.vue`, `views/UsageContainer.vue`, `views/IdentityContainer.vue`, `views/HealthContainer.vue`, `views/KnowledgeContainer.vue`, `views/SkillsContainer.vue`, `views/learning/ReportsContainer.vue`.

**Modified:**
- `src/App.vue` — slimmed to bootstrap + routing + pill.

**Removed:**
- `src/views/KnowledgeView.vue` — replaced by `KnowledgeContainer.vue`.

**Unchanged:** `components/AppShell.vue`, `components/RunFailureList.vue`, all dumb `*View.vue` and their tests.

---

## Task 1: liveStatus store (pure reduce + classify, then registry)

**Files:**
- Create: `crates/right-dashboard/frontend/src/composables/liveStatus.ts`
- Test: `crates/right-dashboard/frontend/src/composables/liveStatus.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/composables/liveStatus.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { DashboardApiError } from '../api'
import { classifyOutcome, reduceConnectionState } from './liveStatus'

describe('reduceConnectionState', () => {
  it('returns null for an empty set', () => {
    expect(reduceConnectionState([])).toBeNull()
  })

  it('shows the worst state by priority locked > offline > stale > loading > live', () => {
    expect(reduceConnectionState(['live', 'live'])).toBe('live')
    expect(reduceConnectionState(['live', 'stale'])).toBe('stale')
    expect(reduceConnectionState(['offline', 'stale', 'live'])).toBe('offline')
    expect(reduceConnectionState(['locked', 'offline'])).toBe('locked')
    expect(reduceConnectionState(['loading', 'live'])).toBe('loading')
  })
})

describe('classifyOutcome', () => {
  it('maps a success to live', () => {
    expect(classifyOutcome({ ok: true, hasData: false })).toBe('live')
  })

  it('maps a 401/403 to locked regardless of data', () => {
    expect(classifyOutcome({ ok: false, error: new DashboardApiError('x', 401), hasData: true })).toBe('locked')
    expect(classifyOutcome({ ok: false, error: new DashboardApiError('x', 403), hasData: false })).toBe('locked')
  })

  it('keeps stale data on a non-auth failure, else offline', () => {
    expect(classifyOutcome({ ok: false, error: new DashboardApiError('x', 500), hasData: true })).toBe('stale')
    expect(classifyOutcome({ ok: false, error: new Error('network'), hasData: false })).toBe('offline')
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `pnpm -C crates/right-dashboard/frontend exec vitest run src/composables/liveStatus.test.ts`
Expected: FAIL — cannot resolve `./liveStatus`.

- [ ] **Step 3: Write the implementation**

Create `src/composables/liveStatus.ts`:

```ts
import { computed, reactive, ref } from 'vue'

import { DashboardApiError } from '../api'

export type ConnectionState = 'loading' | 'live' | 'stale' | 'offline' | 'locked'

const PRIORITY: ConnectionState[] = ['locked', 'offline', 'stale', 'loading', 'live']

export function reduceConnectionState(states: ConnectionState[]): ConnectionState | null {
  if (states.length === 0) {
    return null
  }
  for (const candidate of PRIORITY) {
    if (states.includes(candidate)) {
      return candidate
    }
  }
  return 'live'
}

export function classifyOutcome(args: { ok: boolean, error?: unknown, hasData: boolean }): ConnectionState {
  if (args.ok) {
    return 'live'
  }
  if (args.error instanceof DashboardApiError && args.error.isLocked) {
    return 'locked'
  }
  return args.hasData ? 'stale' : 'offline'
}

const registry = reactive(new Map<string, ConnectionState>())
const sticky = ref<ConnectionState>('loading')

export const globalLastUpdatedAt = ref<string | null>(null)

export const globalConnectionState = computed<ConnectionState>(() => {
  return reduceConnectionState([...registry.values()]) ?? sticky.value
})

export interface LiveStatusHandle {
  report: (state: ConnectionState, at?: string) => void
  dispose: () => void
}

export function registerLiveResource(key: string): LiveStatusHandle {
  registry.set(key, 'loading')
  return {
    report(state: ConnectionState, at?: string): void {
      registry.set(key, state)
      sticky.value = reduceConnectionState([...registry.values()]) ?? state
      if (state === 'live' && at !== undefined) {
        globalLastUpdatedAt.value = at
      }
    },
    dispose(): void {
      registry.delete(key)
    },
  }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `pnpm -C crates/right-dashboard/frontend exec vitest run src/composables/liveStatus.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/composables/liveStatus.ts crates/right-dashboard/frontend/src/composables/liveStatus.test.ts
git commit -m "feat(dashboard): liveStatus store with pure reduce/classify"
```

---

## Task 2: liveConfig (interval via provide/inject)

**Files:**
- Create: `crates/right-dashboard/frontend/src/composables/liveConfig.ts`

No test: this is provide/inject glue with no pure logic (consistent with untested `onMounted` glue elsewhere). `DEFAULT_INTERVAL_MS` is exercised transitively by container typechecks.

- [ ] **Step 1: Write the implementation**

Create `src/composables/liveConfig.ts`:

```ts
import { inject, provide, type InjectionKey, type Ref } from 'vue'

export const DEFAULT_INTERVAL_MS = 5000

export interface LiveConfig {
  intervalMs: number
}

const LiveConfigKey: InjectionKey<Ref<LiveConfig>> = Symbol('liveConfig')

export function provideLiveConfig(config: Ref<LiveConfig>): void {
  provide(LiveConfigKey, config)
}

export function useLiveConfig(): LiveConfig {
  const injected = inject(LiveConfigKey, null)
  return injected !== null ? injected.value : { intervalMs: DEFAULT_INTERVAL_MS }
}
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS (no errors).

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/composables/liveConfig.ts
git commit -m "feat(dashboard): liveConfig provide/inject for poll interval"
```

---

## Task 3: useLiveResource composable (pure shouldTick, then wiring)

**Files:**
- Create: `crates/right-dashboard/frontend/src/composables/useLiveResource.ts`
- Test: `crates/right-dashboard/frontend/src/composables/useLiveResource.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/composables/useLiveResource.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { shouldTick } from './useLiveResource'

describe('shouldTick', () => {
  it('ticks when visible, idle, and not paused', () => {
    expect(shouldTick({ hidden: false, inFlight: false, pauseWhenHidden: true })).toBe(true)
  })

  it('skips while a fetch is already in flight', () => {
    expect(shouldTick({ hidden: false, inFlight: true, pauseWhenHidden: true })).toBe(false)
  })

  it('skips while hidden when pause-when-hidden is on', () => {
    expect(shouldTick({ hidden: true, inFlight: false, pauseWhenHidden: true })).toBe(false)
  })

  it('keeps ticking while hidden when pause-when-hidden is off', () => {
    expect(shouldTick({ hidden: true, inFlight: false, pauseWhenHidden: false })).toBe(true)
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `pnpm -C crates/right-dashboard/frontend exec vitest run src/composables/useLiveResource.test.ts`
Expected: FAIL — cannot resolve `./useLiveResource`.

- [ ] **Step 3: Write the implementation**

Create `src/composables/useLiveResource.ts`:

```ts
import { onBeforeUnmount, onMounted, ref, type Ref } from 'vue'

import { classifyOutcome, registerLiveResource } from './liveStatus'
import { useLiveConfig } from './liveConfig'

export function shouldTick(state: { hidden: boolean, inFlight: boolean, pauseWhenHidden: boolean }): boolean {
  if (state.inFlight) {
    return false
  }
  if (state.pauseWhenHidden && state.hidden) {
    return false
  }
  return true
}

export interface LiveResourceOptions {
  intervalMs?: number
  immediate?: boolean
  pauseWhenHidden?: boolean
  reportConnection?: boolean
  key?: string
}

export interface LiveResource<T> {
  data: Ref<T | null>
  error: Ref<string | null>
  loading: Ref<boolean>
  lastUpdatedAt: Ref<string | null>
  refresh: () => Promise<void>
}

let keySeq = 0

export function useLiveResource<T>(fetcher: () => Promise<T>, options: LiveResourceOptions = {}): LiveResource<T> {
  const config = useLiveConfig()
  const intervalMs = options.intervalMs ?? config.intervalMs
  const immediate = options.immediate ?? true
  const pauseWhenHidden = options.pauseWhenHidden ?? true
  const reportConnection = options.reportConnection ?? true
  const key = options.key ?? `live-${keySeq++}`

  const data = ref(null) as Ref<T | null>
  const error = ref<string | null>(null)
  const loading = ref(false)
  const lastUpdatedAt = ref<string | null>(null)

  const status = reportConnection ? registerLiveResource(key) : null
  let disposed = false
  let inFlight = false
  let generation = 0
  let timer: ReturnType<typeof window.setInterval> | undefined

  async function refresh(): Promise<void> {
    if (disposed || inFlight) {
      return
    }
    inFlight = true
    loading.value = true
    const gen = ++generation
    try {
      const result = await fetcher()
      if (disposed || gen !== generation) {
        return
      }
      data.value = result
      error.value = null
      const at = new Date().toISOString()
      lastUpdatedAt.value = at
      status?.report('live', at)
    } catch (err) {
      if (disposed || gen !== generation) {
        return
      }
      const hasData = data.value !== null
      if (!hasData) {
        error.value = err instanceof Error ? err.message : 'Request failed'
      }
      status?.report(classifyOutcome({ ok: false, error: err, hasData }))
    } finally {
      if (!disposed && gen === generation) {
        loading.value = false
      }
      inFlight = false
    }
  }

  function onVisibility(): void {
    if (!document.hidden) {
      void refresh()
    }
  }

  onMounted(() => {
    if (immediate) {
      void refresh()
    }
    if (intervalMs > 0) {
      timer = window.setInterval(() => {
        if (shouldTick({ hidden: document.hidden, inFlight, pauseWhenHidden })) {
          void refresh()
        }
      }, intervalMs)
    }
    if (pauseWhenHidden) {
      document.addEventListener('visibilitychange', onVisibility)
    }
  })

  onBeforeUnmount(() => {
    disposed = true
    if (timer !== undefined) {
      window.clearInterval(timer)
    }
    document.removeEventListener('visibilitychange', onVisibility)
    status?.dispose()
  })

  return { data, error, loading, lastUpdatedAt, refresh }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `pnpm -C crates/right-dashboard/frontend exec vitest run src/composables/useLiveResource.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 5: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/composables/useLiveResource.ts crates/right-dashboard/frontend/src/composables/useLiveResource.test.ts
git commit -m "feat(dashboard): useLiveResource polling composable"
```

---

## Task 4: activitySelection pure helper

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/activitySelection.ts`
- Test: `crates/right-dashboard/frontend/src/views/activitySelection.test.ts`

- [ ] **Step 1: Write the failing test**

Create `src/views/activitySelection.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import type { OverviewResponse } from '../types'
import { activityContainsRun } from './activitySelection'

function activity(runIds: string[]): OverviewResponse {
  return {
    crons: [{ recent_runs: runIds.map((id) => ({ id })) }],
  } as unknown as OverviewResponse
}

describe('activityContainsRun', () => {
  it('finds a run present in a cron recent_runs list', () => {
    expect(activityContainsRun(activity(['r1', 'r2']), 'r2')).toBe(true)
  })

  it('returns false when the run is absent', () => {
    expect(activityContainsRun(activity(['r1']), 'r9')).toBe(false)
  })

  it('returns false for null activity or null id', () => {
    expect(activityContainsRun(null, 'r1')).toBe(false)
    expect(activityContainsRun(activity(['r1']), null)).toBe(false)
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `pnpm -C crates/right-dashboard/frontend exec vitest run src/views/activitySelection.test.ts`
Expected: FAIL — cannot resolve `./activitySelection`.

- [ ] **Step 3: Write the implementation**

Create `src/views/activitySelection.ts`:

```ts
import type { OverviewResponse } from '../types'

export function activityContainsRun(activity: OverviewResponse | null, runId: string | null): boolean {
  if (activity === null || runId === null) {
    return false
  }
  return activity.crons.some((cron) => cron.recent_runs.some((run) => run.id === runId))
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `pnpm -C crates/right-dashboard/frontend exec vitest run src/views/activitySelection.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/activitySelection.ts crates/right-dashboard/frontend/src/views/activitySelection.test.ts
git commit -m "feat(dashboard): activityContainsRun selection helper"
```

---

## Task 5: UsageContainer

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/UsageContainer.vue`

Containers are thin glue (self-fetch in `onMounted`, which SSR skips), so they are verified by typecheck, not a render test — same status as `McpView`.

- [ ] **Step 1: Write the implementation**

Create `src/views/UsageContainer.vue`:

```vue
<script setup lang="ts">
import { usageOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import UsageView from './UsageView.vue'

const { data, loading, error } = useLiveResource(usageOverview, { key: 'usage' })
</script>

<template>
  <UsageView :usage="data" :loading="loading" :error="error" />
</template>
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/UsageContainer.vue
git commit -m "feat(dashboard): UsageContainer self-fetches via useLiveResource"
```

---

## Task 6: IdentityContainer

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/IdentityContainer.vue`

`IdentityView` props: `identity`, `selectedFile`, `loading` (file-detail load), `error` (file-detail error); emits `selectFile`, `refresh`. The list polls at 30s (near-static files); the `refresh` button forces an immediate refetch.

- [ ] **Step 1: Write the implementation**

Create `src/views/IdentityContainer.vue`:

```vue
<script setup lang="ts">
import { ref, watch } from 'vue'

import { identityFile, identityFiles } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import IdentityView from './IdentityView.vue'
import type { IdentityFileSummary } from '../types'

const { data: identity, refresh } = useLiveResource(identityFiles, { key: 'identity', intervalMs: 30000 })

const selectedFile = ref<IdentityFileSummary | null>(null)
const loadingFile = ref(false)
const fileError = ref<string | null>(null)

watch(identity, (value) => {
  if (selectedFile.value === null && value !== null) {
    selectedFile.value = value.files[0] ?? null
  }
})

async function selectFile(name: string): Promise<void> {
  loadingFile.value = true
  fileError.value = null
  try {
    const response = await identityFile(name)
    selectedFile.value = response.file
    if (identity.value !== null) {
      identity.value.warning = response.warning ?? identity.value.warning
      identity.value.files = identity.value.files.map((file) => (file.name === name ? response.file : file))
    }
  } catch (err) {
    fileError.value = err instanceof Error ? err.message : 'Identity file unavailable'
  } finally {
    loadingFile.value = false
  }
}
</script>

<template>
  <IdentityView
    :identity="identity"
    :selected-file="selectedFile"
    :loading="loadingFile"
    :error="fileError"
    @select-file="selectFile"
    @refresh="refresh"
  />
</template>
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/IdentityContainer.vue
git commit -m "feat(dashboard): IdentityContainer (30s poll + file detail)"
```

---

## Task 7: ReportsContainer (Knowledge → Learning subtab)

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/learning/ReportsContainer.vue`

`ReportsView` props: `learning` only. Note the path is `views/learning/`, so imports go up two levels.

- [ ] **Step 1: Write the implementation**

Create `src/views/learning/ReportsContainer.vue`:

```vue
<script setup lang="ts">
import { learningOverview } from '../../api'
import { useLiveResource } from '../../composables/useLiveResource'
import ReportsView from './ReportsView.vue'

const { data } = useLiveResource(learningOverview, { key: 'learning' })
</script>

<template>
  <ReportsView :learning="data" />
</template>
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/learning/ReportsContainer.vue
git commit -m "feat(dashboard): ReportsContainer self-fetches learning overview"
```

---

## Task 8: SkillsContainer (Knowledge → Skills subtab)

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/SkillsContainer.vue`

`SkillsView` props: `skills`, `selectedSkill`, `selectedSkillName`, `loading` (detail), `error` (detail); emits `selectSkill`, `skillPinned`. Pin mutation is fired inside `SkillsView` (it imports `setSkillPinned`); the container applies the emitted `skillPinned` result to the list + selected skill (verbatim from the old `App.vue::applySkillPinned`).

- [ ] **Step 1: Write the implementation**

Create `src/views/SkillsContainer.vue`:

```vue
<script setup lang="ts">
import { ref } from 'vue'

import { skillDetail, skillsOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import SkillsView from './SkillsView.vue'
import type { SkillDetailResponse, SkillSummary } from '../types'

const { data: skills } = useLiveResource(skillsOverview, { key: 'skills' })

const selectedSkill = ref<SkillDetailResponse | null>(null)
const selectedSkillName = ref<string | null>(null)
const loadingSkill = ref(false)
const skillError = ref<string | null>(null)

async function selectSkill(skill: SkillSummary): Promise<void> {
  selectedSkillName.value = skill.name
  selectedSkill.value = null
  loadingSkill.value = true
  skillError.value = null
  try {
    const detail = await skillDetail(skill.name)
    if (selectedSkillName.value === skill.name) {
      selectedSkill.value = detail
    }
  } catch (err) {
    if (selectedSkillName.value === skill.name) {
      skillError.value = err instanceof Error ? err.message : 'Skill unavailable'
    }
  } finally {
    if (selectedSkillName.value === skill.name) {
      loadingSkill.value = false
    }
  }
}

function applySkillPinned({ skillName, pinned }: { skillName: string, pinned: boolean }): void {
  if (selectedSkill.value && selectedSkill.value.skill.name === skillName) {
    selectedSkill.value = {
      ...selectedSkill.value,
      skill: { ...selectedSkill.value.skill, pinned },
    }
  }
  const current = skills.value
  if (current === null) {
    return
  }
  const updateGroup = (group: SkillSummary[]): SkillSummary[] =>
    group.map((skill) => (skill.name === skillName ? { ...skill, pinned } : skill))
  skills.value = {
    ...current,
    groups: {
      core: updateGroup(current.groups.core),
      learned: updateGroup(current.groups.learned),
      other: updateGroup(current.groups.other),
    },
  }
}
</script>

<template>
  <SkillsView
    :skills="skills"
    :selected-skill="selectedSkill"
    :selected-skill-name="selectedSkillName"
    :loading="loadingSkill"
    :error="skillError"
    @select-skill="selectSkill"
    @skill-pinned="applySkillPinned"
  />
</template>
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/SkillsContainer.vue
git commit -m "feat(dashboard): SkillsContainer (poll + skill detail + pin)"
```

---

## Task 9: KnowledgeContainer (subtab nav + sub-containers)

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/KnowledgeContainer.vue`

Owns the subtab state and routes to the two sub-containers by `v-if` (so the inactive subtab unmounts and stops polling). The nav markup is moved verbatim from the old dumb `KnowledgeView.vue`.

- [ ] **Step 1: Write the implementation**

Create `src/views/KnowledgeContainer.vue`:

```vue
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
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/KnowledgeContainer.vue
git commit -m "feat(dashboard): KnowledgeContainer routes learning/skills subtabs"
```

---

## Task 10: OverviewContainer

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/OverviewContainer.vue`

`OverviewView` props: `overview` (dashboard overview), `activity` (activity overview), `loading`, `error`. It has no App-driven run selection (its failures card uses the self-contained `RunFailureList`). Two resources; map the dashboard-overview resource's `loading`/`error` to the view's single `loading`/`error` pair. Note `api.ts` exports the activity endpoint as `overview`; import it aliased.

- [ ] **Step 1: Write the implementation**

Create `src/views/OverviewContainer.vue`:

```vue
<script setup lang="ts">
import { dashboardOverview, overview as activityOverview } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import OverviewView from './OverviewView.vue'

const { data: overviewData, loading, error } = useLiveResource(dashboardOverview, { key: 'overview' })
const { data: activityData } = useLiveResource(activityOverview, { key: 'overview-activity' })
</script>

<template>
  <OverviewView
    :overview="overviewData"
    :activity="activityData"
    :loading="loading"
    :error="error"
  />
</template>
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/OverviewContainer.vue
git commit -m "feat(dashboard): OverviewContainer (overview + activity resources)"
```

---

## Task 11: ActivityContainer

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/ActivityContainer.vue`

`ActivityView` props: `overview` (activity), `selectedRun`, `selectedRunId`, `loadingDetail`, `detailError`; emits `selectRun`. The container owns the main-list run selection + `runDetail` fetch (moved verbatim from old `App.vue`) and clears the selection when a poll drops the run, via `activityContainsRun` in a `watch`.

- [ ] **Step 1: Write the implementation**

Create `src/views/ActivityContainer.vue`:

```vue
<script setup lang="ts">
import { ref, watch } from 'vue'

import { overview as activityOverview, runDetail } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import { activityContainsRun } from './activitySelection'
import ActivityView from './ActivityView.vue'
import type { RunDetailResponse, RunSummary } from '../types'

const { data: activity } = useLiveResource(activityOverview, { key: 'activity' })

const selectedRun = ref<RunDetailResponse | null>(null)
const selectedRunId = ref<string | null>(null)
const loadingDetail = ref(false)
const detailError = ref<string | null>(null)

async function selectRun(run: RunSummary): Promise<void> {
  const runId = run.id
  selectedRunId.value = runId
  selectedRun.value = null
  loadingDetail.value = true
  detailError.value = null
  try {
    const detail = await runDetail(runId)
    if (selectedRunId.value === runId) {
      selectedRun.value = detail
    }
  } catch (err) {
    if (selectedRunId.value === runId) {
      detailError.value = err instanceof Error ? err.message : 'Run unavailable'
    }
  } finally {
    if (selectedRunId.value === runId) {
      loadingDetail.value = false
    }
  }
}

watch(activity, (value) => {
  if (selectedRunId.value !== null && !activityContainsRun(value, selectedRunId.value)) {
    selectedRunId.value = null
    selectedRun.value = null
    detailError.value = null
  }
})
</script>

<template>
  <ActivityView
    :overview="activity"
    :selected-run="selectedRun"
    :selected-run-id="selectedRunId"
    :loading-detail="loadingDetail"
    :detail-error="detailError"
    @select-run="selectRun"
  />
</template>
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/ActivityContainer.vue
git commit -m "feat(dashboard): ActivityContainer (poll + run detail + reconcile)"
```

---

## Task 12: HealthContainer (manual, no poll)

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/HealthContainer.vue`

`HealthView` props: `doctor`, `sandbox`, `loadingDoctor`, `loadingSandbox`, `doctorError`, `sandboxError`; emits `refreshDoctor`, `refreshSandbox`. Both resources are manual (`immediate: false`, `intervalMs: 0`) and do not report connection (`reportConnection: false`) — doctor/sandbox are expensive gRPC and must not run implicitly (ARCHITECTURE.md). The buttons call `refresh()`.

- [ ] **Step 1: Write the implementation**

Create `src/views/HealthContainer.vue`:

```vue
<script setup lang="ts">
import { doctorStatus, sandboxStats } from '../api'
import { useLiveResource } from '../composables/useLiveResource'
import HealthView from './HealthView.vue'

const manual = { immediate: false, intervalMs: 0, reportConnection: false }

const { data: doctor, loading: loadingDoctor, error: doctorError, refresh: refreshDoctor } =
  useLiveResource(doctorStatus, { ...manual, key: 'doctor' })
const { data: sandbox, loading: loadingSandbox, error: sandboxError, refresh: refreshSandbox } =
  useLiveResource(sandboxStats, { ...manual, key: 'sandbox' })
</script>

<template>
  <HealthView
    :doctor="doctor"
    :sandbox="sandbox"
    :loading-doctor="loadingDoctor"
    :loading-sandbox="loadingSandbox"
    :doctor-error="doctorError"
    :sandbox-error="sandboxError"
    @refresh-doctor="refreshDoctor"
    @refresh-sandbox="refreshSandbox"
  />
</template>
```

- [ ] **Step 2: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/views/HealthContainer.vue
git commit -m "feat(dashboard): HealthContainer (manual doctor/sandbox refresh)"
```

---

## Task 13: App.vue cutover + remove KnowledgeView

**Files:**
- Modify (replace whole file): `crates/right-dashboard/frontend/src/App.vue`
- Delete: `crates/right-dashboard/frontend/src/views/KnowledgeView.vue`

`App.vue` drops all data ownership (every `*Data` ref, `schedulePolling`, `pollTimer`, `guarded`, `applyErrorState`, all `refresh*`, all `select*`, all detail-loading refs, the `ConnectionState`/`KnowledgeTab` types) and becomes: bootstrap (load-once via `useLiveResource`), `provideLiveConfig`, tab state + display mode + Telegram init, and the pill fed from `liveStatus`. It renders containers.

- [ ] **Step 1: Replace the `<script setup>` and `<template>` sections of `src/App.vue`**

Replace ONLY the `<script setup> … </script>` block and the `<template> … </template>` block. **Leave the existing `<style> … </style>` block (≈580 lines of shared dashboard CSS that every view depends on) untouched** — do not overwrite the whole file. New script + template:

```vue
<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'

import { bootstrap } from './api'
import { useLiveResource } from './composables/useLiveResource'
import { provideLiveConfig } from './composables/liveConfig'
import { globalConnectionState, globalLastUpdatedAt } from './composables/liveStatus'
import {
  dashboardTabItems,
  isDashboardTab,
  normalizeInitialTab,
  type DashboardTab,
} from './dashboardTabs'
import { initialDashboardTabFromLocation } from './format'
import {
  applyTelegramDisplayMode,
  initializeTelegramWebApp,
  nextDashboardDisplayModePreference,
  readDashboardDisplayMode,
  subscribeTelegramFullscreenChanges,
  type DashboardDisplayMode,
} from './telegram'
import AppShell from './components/AppShell.vue'
import OverviewContainer from './views/OverviewContainer.vue'
import ActivityContainer from './views/ActivityContainer.vue'
import KnowledgeContainer from './views/KnowledgeContainer.vue'
import UsageContainer from './views/UsageContainer.vue'
import IdentityContainer from './views/IdentityContainer.vue'
import HealthContainer from './views/HealthContainer.vue'
import McpView from './views/McpView.vue'
import ProvidersView from './views/ProvidersView.vue'

const { data: bootstrapData } = useLiveResource(bootstrap, { intervalMs: 0, key: 'bootstrap' })

const activeTab = ref<DashboardTab>(
  normalizeInitialTab(initialDashboardTabFromLocation(window.location.search, window.location.hash)),
)
const preferredDisplayMode = ref<DashboardDisplayMode>(readDashboardDisplayMode())
const displayMode = ref<DashboardDisplayMode>('normal')

const refreshIntervalMs = computed(() => Math.max(bootstrapData.value?.refresh_interval_secs ?? 5, 1) * 1000)
provideLiveConfig(computed(() => ({ intervalMs: refreshIntervalMs.value })))

const shellTitle = computed(() => bootstrapData.value?.agent ?? 'Dashboard')
const tabs = computed(() => dashboardTabItems(bootstrapData.value?.features))

const connectionMessage = computed(() => {
  switch (globalConnectionState.value) {
    case 'live':
      return 'Live'
    case 'stale':
      return 'Reconnecting'
    case 'offline':
      return 'Dashboard unavailable'
    case 'locked':
      return 'Dashboard locked'
    default:
      return 'Loading dashboard'
  }
})

let unsubscribeTelegramFullscreen: (() => void) | undefined

onMounted(() => {
  const webApp = window.Telegram?.WebApp
  displayMode.value = initializeTelegramWebApp(webApp, preferredDisplayMode.value)
  unsubscribeTelegramFullscreen = subscribeTelegramFullscreenChanges(webApp, (mode) => {
    displayMode.value = mode
  })
})

onBeforeUnmount(() => {
  unsubscribeTelegramFullscreen?.()
})

function setActiveTab(tab: string): void {
  if (!isDashboardTab(tab)) {
    return
  }
  activeTab.value = tab
}

function toggleDisplayMode(): void {
  const nextMode = nextDashboardDisplayModePreference(preferredDisplayMode.value)
  preferredDisplayMode.value = nextMode
  displayMode.value = applyTelegramDisplayMode(nextMode)
}
</script>

<template>
  <AppShell
    :agent="shellTitle"
    :connection-state="globalConnectionState"
    :message="connectionMessage"
    :last-updated-at="globalLastUpdatedAt"
    :tabs="tabs"
    :active-tab="activeTab"
    :display-mode="displayMode"
    :preferred-display-mode="preferredDisplayMode"
    @select="setActiveTab"
    @toggle-display-mode="toggleDisplayMode"
  >
    <OverviewContainer v-if="activeTab === 'overview'" />
    <ActivityContainer v-else-if="activeTab === 'activity'" />
    <KnowledgeContainer v-else-if="activeTab === 'knowledge'" />
    <UsageContainer v-else-if="activeTab === 'usage'" />
    <IdentityContainer v-else-if="activeTab === 'identity'" />
    <McpView v-else-if="activeTab === 'mcp'" />
    <ProvidersView v-else-if="activeTab === 'providers'" />
    <HealthContainer v-else-if="activeTab === 'health'" />
    <section v-else class="empty-panel">Unknown dashboard view</section>
  </AppShell>
</template>
```

> **IMPORTANT:** The snippet above stops at `</template>` on purpose. Do not touch the file's existing `<style> … </style>` block — it holds the shared dashboard CSS all views rely on. Editing tools should match-and-replace the old `<script setup>`/`<template>` blocks, not rewrite the file.

- [ ] **Step 2: Delete the dumb KnowledgeView**

Run: `git rm crates/right-dashboard/frontend/src/views/KnowledgeView.vue`
Expected: file removed (no other file imports it after Step 1).

- [ ] **Step 3: Typecheck**

Run: `pnpm -C crates/right-dashboard/frontend typecheck`
Expected: PASS. If it flags an unused import or a missing prop, fix the App.vue script/template accordingly.

- [ ] **Step 4: Run the full frontend test suite (dumb-view tests must still pass)**

Run: `pnpm -C crates/right-dashboard/frontend test`
Expected: PASS — all existing `*View.test.ts`, `App.test.ts`, component tests, plus the new composable/helper tests.

- [ ] **Step 5: Build (vue-tsc + vite)**

Run: `pnpm -C crates/right-dashboard/frontend build`
Expected: PASS — the SPA compiles.

- [ ] **Step 6: Commit**

```bash
cd /Users/developer/dev/rightclaw
test "$(git rev-parse --abbrev-ref HEAD)" = master || { echo "not on master"; exit 1; }
git add crates/right-dashboard/frontend/src/App.vue
git rm crates/right-dashboard/frontend/src/views/KnowledgeView.vue
git commit -m "feat(dashboard): cut App.vue over to live containers"
```

---

## Task 14: Final verification

- [ ] **Step 1: Full frontend test + typecheck + build**

```bash
pnpm -C crates/right-dashboard/frontend test
pnpm -C crates/right-dashboard/frontend typecheck
pnpm -C crates/right-dashboard/frontend build
```
Expected: all PASS.

- [ ] **Step 2: Full workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. This compiles `right-dashboard` (its `build.rs` runs `vite build`), proving the embedded SPA builds inside the Rust build.

- [ ] **Step 3: Manual smoke (optional but recommended)**

Open the dashboard Mini App, leave it on the Usage tab, trigger a usage-producing action, and confirm the numbers update within the poll interval without reloading. Switch to another app/tab (hide the webview) and back — confirm an immediate catch-up refresh. Open the Health tab — confirm doctor/sandbox stay blank until the Refresh button is pressed.

---

## Self-review notes (already reconciled)

- **Spec coverage:** every spec section maps to a task — `useLiveResource` (T3), `liveStatus` (T1), `liveConfig` (T2), run-detail-no-shared-composable + `activityContainsRun` (T4, T11), the eight containers (T5–T12), App.vue slim-down + KnowledgeView removal (T13), testing + final workspace test (T14). Health-stays-manual is T12; "no dedicated heartbeat" is honored (App.vue holds no always-on resource — only load-once bootstrap).
- **Type consistency:** `useLiveResource` returns `{ data, error, loading, lastUpdatedAt, refresh }` used identically by all containers; `classifyOutcome`/`reduceConnectionState`/`ConnectionState` are defined in T1 and consumed in T3/App.vue; `activityContainsRun(OverviewResponse | null, string | null)` defined T4, used T11.
- **MCP/Providers** stay as-is (self-fetching) — out of scope per spec; App.vue still renders `McpView`/`ProvidersView` directly.
