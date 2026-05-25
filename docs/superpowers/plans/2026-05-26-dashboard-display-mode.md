# Dashboard Display Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Telegram Mini App dashboard default to normal mode, offer an explicit fullscreen/normal toggle, remember the last preferred display mode locally, and keep normal-mode navigation compact.

**Architecture:** Keep the feature inside `crates/right-dashboard/frontend`. `telegram.ts` owns Telegram WebApp and localStorage boundaries; `App.vue` owns reactive display/layout mode and lifecycle cleanup; `AppShell.vue` renders the topbar control and adaptive navigation. No backend API, DTO, database, or Rust code changes are expected.

**Tech Stack:** Vue 3 `<script setup>`, TypeScript, Vitest, Vite, Telegram Mini App WebApp API, browser `localStorage`, existing project CSS.

---

## File Structure

- Modify: `crates/right-dashboard/frontend/src/telegram.ts`
  - Owns display-mode type, storage key, safe localStorage parsing/persistence, Telegram fullscreen request/exit guards, initialization, and fullscreen event subscription cleanup.
- Modify: `crates/right-dashboard/frontend/src/telegram.test.ts`
  - Unit tests for default normal startup, saved fullscreen startup, invalid storage values, storage failures, request/exit behavior, synchronous fullscreen failures, and fullscreen event subscription cleanup.
- Modify: `crates/right-dashboard/frontend/src/App.vue`
  - Owns `displayMode`, initializes Telegram from the saved preference, handles topbar display-mode toggles, subscribes/unsubscribes to Telegram fullscreen events, and passes mode to the shell.
- Modify: `crates/right-dashboard/frontend/src/components/AppShell.vue`
  - Accepts `displayMode`, emits `toggle-display-mode`, renders the display-mode button, and tags shell/nav markup for adaptive CSS.
- Check: `docs/architecture/modules.md`
  - Re-read because dashboard frontend is touched. No update is expected unless implementation changes the documented dashboard ownership or file map.
- Generated after build: `crates/right-dashboard/static/dashboard/`
  - Commit generated static assets if `npm run build --prefix crates/right-dashboard/frontend` changes them.

---

### Task 1: Baseline And Architecture Check

**Files:**
- Read: `docs/architecture/modules.md`
- Read: `docs/superpowers/specs/2026-05-26-dashboard-display-mode-design.md`
- No code changes.

- [ ] **Step 1: Confirm working tree state**

Run:

```sh
devenv shell -- git status --short
```

Expected: no unrelated tracked changes. If unrelated changes exist, leave them untouched and mention them before editing.

- [ ] **Step 2: Re-read dashboard architecture docs**

Run:

```sh
devenv shell -- sed -n '78,94p' docs/architecture/modules.md
```

Expected: the `right-dashboard` section still states that `frontend/` is Vue/Vite source and `static/dashboard/` is checked-in generated output. This feature stays within that boundary, so no architecture doc update should be needed.

- [ ] **Step 3: Re-read the approved spec**

Run:

```sh
devenv shell -- sed -n '1,240p' docs/superpowers/specs/2026-05-26-dashboard-display-mode-design.md
```

Expected: confirms frontend-only local preference, normal default, bottom horizontally scrollable normal-mode nav, and final verification requirements.

- [ ] **Step 4: Run frontend baseline tests**

Run:

```sh
devenv shell -- npm run test --prefix crates/right-dashboard/frontend
```

Expected: current frontend tests pass before edits.

---

### Task 2: Telegram Display Mode Helper

**Files:**
- Modify: `crates/right-dashboard/frontend/src/telegram.test.ts`
- Modify: `crates/right-dashboard/frontend/src/telegram.ts`

- [ ] **Step 1: Replace the Telegram tests with failing behavior tests**

Replace `crates/right-dashboard/frontend/src/telegram.test.ts` with:

```ts
import { describe, expect, test, vi } from 'vitest'

import {
  DASHBOARD_DISPLAY_MODE_STORAGE_KEY,
  applyTelegramDisplayMode,
  initializeTelegramWebApp,
  readDashboardDisplayMode,
  saveDashboardDisplayMode,
  subscribeTelegramFullscreenChanges,
  type DashboardDisplayMode,
  type TelegramWebApp,
} from './telegram'

class MemoryStorage implements Storage {
  private values = new Map<string, string>()

  get length(): number {
    return this.values.size
  }

  clear(): void {
    this.values.clear()
  }

  getItem(key: string): string | null {
    return this.values.get(key) ?? null
  }

  key(index: number): string | null {
    return Array.from(this.values.keys())[index] ?? null
  }

  removeItem(key: string): void {
    this.values.delete(key)
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value)
  }
}

class ThrowingStorage extends MemoryStorage {
  override getItem(): string | null {
    throw new Error('storage unavailable')
  }

  override setItem(): void {
    throw new Error('storage unavailable')
  }
}

describe('dashboard display mode storage', () => {
  test('defaults to normal when no preference is stored', () => {
    const storage = new MemoryStorage()

    expect(readDashboardDisplayMode(storage)).toBe('normal')
  })

  test('reads a saved fullscreen preference', () => {
    const storage = new MemoryStorage()
    storage.setItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, 'fullscreen')

    expect(readDashboardDisplayMode(storage)).toBe('fullscreen')
  })

  test('treats invalid values as normal', () => {
    const storage = new MemoryStorage()
    storage.setItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, 'wide')

    expect(readDashboardDisplayMode(storage)).toBe('normal')
  })

  test('ignores storage read and write failures', () => {
    const storage = new ThrowingStorage()

    expect(readDashboardDisplayMode(storage)).toBe('normal')
    expect(() => saveDashboardDisplayMode('fullscreen', storage)).not.toThrow()
  })
})

describe('initializeTelegramWebApp', () => {
  test('readies and expands without requesting fullscreen by default', () => {
    const calls: string[] = []
    const webApp = {
      ready: vi.fn(() => calls.push('ready')),
      requestFullscreen: vi.fn(() => calls.push('requestFullscreen')),
      expand: vi.fn(() => calls.push('expand')),
    }

    const mode = initializeTelegramWebApp(webApp, 'normal')

    expect(mode).toBe('normal')
    expect(calls).toEqual(['ready', 'expand'])
  })

  test('requests fullscreen only when the saved preference is fullscreen', () => {
    const calls: string[] = []
    const webApp = {
      ready: vi.fn(() => calls.push('ready')),
      requestFullscreen: vi.fn(() => calls.push('requestFullscreen')),
      expand: vi.fn(() => calls.push('expand')),
    }

    const mode = initializeTelegramWebApp(webApp, 'fullscreen')

    expect(mode).toBe('fullscreen')
    expect(calls).toEqual(['ready', 'expand', 'requestFullscreen'])
  })

  test('returns normal layout when a saved fullscreen request throws synchronously', () => {
    const webApp = {
      ready: vi.fn(),
      requestFullscreen: vi.fn(() => {
        throw new Error('fullscreen unavailable')
      }),
      expand: vi.fn(),
      isFullscreen: false,
    }

    expect(() => initializeTelegramWebApp(webApp, 'fullscreen')).not.toThrow()
    expect(initializeTelegramWebApp(webApp, 'fullscreen')).toBe('normal')
  })
})

describe('applyTelegramDisplayMode', () => {
  test('persists fullscreen preference and requests fullscreen', () => {
    const storage = new MemoryStorage()
    const webApp = {
      requestFullscreen: vi.fn(),
      exitFullscreen: vi.fn(),
    }

    const mode = applyTelegramDisplayMode('fullscreen', webApp, storage)

    expect(mode).toBe('fullscreen')
    expect(storage.getItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY)).toBe('fullscreen')
    expect(webApp.requestFullscreen).toHaveBeenCalledOnce()
    expect(webApp.exitFullscreen).not.toHaveBeenCalled()
  })

  test('persists normal preference and exits fullscreen', () => {
    const storage = new MemoryStorage()
    const webApp = {
      requestFullscreen: vi.fn(),
      exitFullscreen: vi.fn(),
    }

    const mode = applyTelegramDisplayMode('normal', webApp, storage)

    expect(mode).toBe('normal')
    expect(storage.getItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY)).toBe('normal')
    expect(webApp.exitFullscreen).toHaveBeenCalledOnce()
    expect(webApp.requestFullscreen).not.toHaveBeenCalled()
  })

  test('keeps saved preference but returns actual normal layout when fullscreen throws', () => {
    const storage = new MemoryStorage()
    const webApp = {
      isFullscreen: false,
      requestFullscreen: vi.fn(() => {
        throw new Error('fullscreen unavailable')
      }),
    }

    const mode = applyTelegramDisplayMode('fullscreen', webApp, storage)

    expect(mode).toBe('normal')
    expect(storage.getItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY)).toBe('fullscreen')
  })
})

describe('subscribeTelegramFullscreenChanges', () => {
  test('maps Telegram fullscreen_changed events to dashboard display mode', () => {
    let eventHandler: ((event: unknown) => void) | undefined
    const changes: DashboardDisplayMode[] = []
    const webApp: TelegramWebApp = {
      onEvent: vi.fn((eventType: string, handler: (...args: unknown[]) => void) => {
        if (eventType === 'fullscreen_changed') {
          eventHandler = handler
        }
      }),
      offEvent: vi.fn(),
    }

    const unsubscribe = subscribeTelegramFullscreenChanges(webApp, (mode) => changes.push(mode))
    eventHandler?.({ is_fullscreen: true })
    eventHandler?.({ is_fullscreen: false })
    unsubscribe()

    expect(changes).toEqual(['fullscreen', 'normal'])
    expect(webApp.offEvent).toHaveBeenCalledWith('fullscreen_changed', eventHandler)
  })
})
```

- [ ] **Step 2: Run tests and verify they fail for the expected reason**

Run:

```sh
devenv shell -- npm run test --prefix crates/right-dashboard/frontend
```

Expected: FAIL because `telegram.ts` does not export the new display-mode helpers and still requests fullscreen unconditionally.

- [ ] **Step 3: Replace `telegram.ts` with the display-mode helper implementation**

Replace `crates/right-dashboard/frontend/src/telegram.ts` with:

```ts
export type DashboardDisplayMode = 'normal' | 'fullscreen'

export const DASHBOARD_DISPLAY_MODE_STORAGE_KEY = 'right-dashboard.display-mode'

type TelegramEventHandler = (...args: unknown[]) => void

export interface TelegramWebApp {
  initData?: string
  ready?: () => void
  requestFullscreen?: () => void
  exitFullscreen?: () => void
  expand?: () => void
  isFullscreen?: boolean
  onEvent?: (eventType: string, eventHandler: TelegramEventHandler) => void
  offEvent?: (eventType: string, eventHandler: TelegramEventHandler) => void
}

declare global {
  interface Window {
    Telegram?: {
      WebApp?: TelegramWebApp
    }
  }
}

function normalizeDisplayMode(value: string | null | undefined): DashboardDisplayMode {
  return value === 'fullscreen' ? 'fullscreen' : 'normal'
}

function defaultStorage(): Storage | undefined {
  try {
    return window.localStorage
  } catch {
    return undefined
  }
}

export function readDashboardDisplayMode(storage: Storage | undefined = defaultStorage()): DashboardDisplayMode {
  try {
    return normalizeDisplayMode(storage?.getItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY))
  } catch {
    return 'normal'
  }
}

export function saveDashboardDisplayMode(
  mode: DashboardDisplayMode,
  storage: Storage | undefined = defaultStorage(),
): void {
  try {
    storage?.setItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, mode)
  } catch {
    // Display mode is a best-effort local preference; storage failures must not break startup.
  }
}

function actualDisplayMode(webApp: TelegramWebApp | undefined): DashboardDisplayMode {
  return webApp?.isFullscreen === true ? 'fullscreen' : 'normal'
}

export function initializeTelegramWebApp(
  webApp: TelegramWebApp | undefined = window.Telegram?.WebApp,
  preferredMode: DashboardDisplayMode = readDashboardDisplayMode(),
): DashboardDisplayMode {
  const mode = normalizeDisplayMode(preferredMode)
  webApp?.ready?.()
  webApp?.expand?.()

  if (mode !== 'fullscreen') {
    return actualDisplayMode(webApp)
  }

  try {
    webApp?.requestFullscreen?.()
    return 'fullscreen'
  } catch {
    return actualDisplayMode(webApp)
  }
}

export function applyTelegramDisplayMode(
  mode: DashboardDisplayMode,
  webApp: TelegramWebApp | undefined = window.Telegram?.WebApp,
  storage: Storage | undefined = defaultStorage(),
): DashboardDisplayMode {
  saveDashboardDisplayMode(mode, storage)
  try {
    if (mode === 'fullscreen') {
      webApp?.requestFullscreen?.()
    } else {
      webApp?.exitFullscreen?.()
    }
    return mode
  } catch {
    return actualDisplayMode(webApp)
  }
}

function fullscreenChangedMode(webApp: TelegramWebApp | undefined, event: unknown): DashboardDisplayMode {
  if (event !== null && typeof event === 'object' && 'is_fullscreen' in event) {
    return Boolean((event as { is_fullscreen?: unknown }).is_fullscreen) ? 'fullscreen' : 'normal'
  }
  return actualDisplayMode(webApp)
}

export function subscribeTelegramFullscreenChanges(
  webApp: TelegramWebApp | undefined,
  onChange: (mode: DashboardDisplayMode) => void,
): () => void {
  if (webApp?.onEvent === undefined || webApp.offEvent === undefined) {
    return () => {}
  }

  const handler: TelegramEventHandler = (event) => {
    onChange(fullscreenChangedMode(webApp, event))
  }

  webApp.onEvent('fullscreen_changed', handler)
  return () => webApp.offEvent?.('fullscreen_changed', handler)
}
```

- [ ] **Step 4: Run the targeted frontend tests**

Run:

```sh
devenv shell -- npm run test --prefix crates/right-dashboard/frontend
```

Expected: PASS. The old unconditional fullscreen behavior is gone, and the new helper behavior is covered.

- [ ] **Step 5: Commit Task 2**

Run:

```sh
devenv shell -- git add crates/right-dashboard/frontend/src/telegram.ts crates/right-dashboard/frontend/src/telegram.test.ts
devenv shell -- git commit -m "feat(dashboard): persist display mode preference"
```

Expected: commit succeeds.

---

### Task 3: App State And Shell Control

**Files:**
- Modify: `crates/right-dashboard/frontend/src/App.vue`
- Modify: `crates/right-dashboard/frontend/src/components/AppShell.vue`

- [ ] **Step 1: Update `App.vue` imports**

In `crates/right-dashboard/frontend/src/App.vue`, replace:

```ts
import { initializeTelegramWebApp } from './telegram'
```

with:

```ts
import {
  applyTelegramDisplayMode,
  initializeTelegramWebApp,
  readDashboardDisplayMode,
  subscribeTelegramFullscreenChanges,
  type DashboardDisplayMode,
} from './telegram'
```

- [ ] **Step 2: Add display-mode state and subscription storage**

In `crates/right-dashboard/frontend/src/App.vue`, after:

```ts
const lastUpdatedAt = ref<string | null>(null)
```

add:

```ts
const displayMode = ref<DashboardDisplayMode>(readDashboardDisplayMode())
```

Replace:

```ts
let pollTimer: number | undefined
```

with:

```ts
let pollTimer: number | undefined
let unsubscribeTelegramFullscreen: (() => void) | undefined
```

- [ ] **Step 3: Initialize Telegram display mode and subscribe to fullscreen events**

In `crates/right-dashboard/frontend/src/App.vue`, replace the current mounted hook:

```ts
onMounted(() => {
  initializeTelegramWebApp()
  void loadInitial()
})
```

with:

```ts
onMounted(() => {
  const webApp = window.Telegram?.WebApp
  displayMode.value = initializeTelegramWebApp(webApp, displayMode.value)
  unsubscribeTelegramFullscreen = subscribeTelegramFullscreenChanges(webApp, (mode) => {
    displayMode.value = mode
  })
  void loadInitial()
})
```

- [ ] **Step 4: Clean up fullscreen event subscription**

In `crates/right-dashboard/frontend/src/App.vue`, replace the current unmount hook:

```ts
onBeforeUnmount(() => {
  if (pollTimer !== undefined) {
    window.clearInterval(pollTimer)
  }
})
```

with:

```ts
onBeforeUnmount(() => {
  if (pollTimer !== undefined) {
    window.clearInterval(pollTimer)
  }
  unsubscribeTelegramFullscreen?.()
})
```

- [ ] **Step 5: Add the display-mode toggle handler**

In `crates/right-dashboard/frontend/src/App.vue`, after `isDashboardTab`, add:

```ts
function toggleDisplayMode(): void {
  const nextMode: DashboardDisplayMode = displayMode.value === 'fullscreen' ? 'normal' : 'fullscreen'
  displayMode.value = applyTelegramDisplayMode(nextMode)
}
```

- [ ] **Step 6: Pass mode and toggle event into `AppShell`**

In `crates/right-dashboard/frontend/src/App.vue`, change the `AppShell` opening tag from:

```vue
  <AppShell
    :agent="shellTitle"
    :connection-state="connectionState"
    :message="message"
    :last-updated-at="lastUpdatedAt"
    :tabs="tabs"
    :active-tab="activeTab"
    @select="setActiveTab"
  >
```

to:

```vue
  <AppShell
    :agent="shellTitle"
    :connection-state="connectionState"
    :message="message"
    :last-updated-at="lastUpdatedAt"
    :tabs="tabs"
    :active-tab="activeTab"
    :display-mode="displayMode"
    @select="setActiveTab"
    @toggle-display-mode="toggleDisplayMode"
  >
```

- [ ] **Step 7: Replace `AppShell.vue` with the display-mode shell markup**

Replace `crates/right-dashboard/frontend/src/components/AppShell.vue` with:

```vue
<script setup lang="ts">
import type { DashboardDisplayMode } from '../telegram'
import { shortDate } from '../format'
import StatusPill from './StatusPill.vue'

export interface ShellTab {
  key: string
  label: string
  enabled: boolean
}

defineProps<{
  agent: string
  connectionState: string
  message: string
  lastUpdatedAt: string | null
  tabs: ShellTab[]
  activeTab: string
  displayMode: DashboardDisplayMode
}>()

const emit = defineEmits<{
  select: [tab: string]
  toggleDisplayMode: []
}>()
</script>

<template>
  <main class="app-shell" :class="`display-${displayMode}`">
    <header class="topbar">
      <div>
        <p class="eyebrow">Right Agent</p>
        <h1>{{ agent }}</h1>
      </div>
      <div class="topbar-actions">
        <button
          type="button"
          class="display-mode-button"
          :aria-label="displayMode === 'fullscreen' ? 'Use normal view' : 'Use fullscreen view'"
          @click="emit('toggleDisplayMode')"
        >
          {{ displayMode === 'fullscreen' ? 'Normal' : 'Fullscreen' }}
        </button>
        <StatusPill :status="connectionState" />
      </div>
    </header>

    <section v-if="connectionState !== 'live'" class="notice" :class="connectionState">
      <strong>{{ message }}</strong>
      <span v-if="lastUpdatedAt">Last update {{ shortDate(lastUpdatedAt) }}</span>
    </section>

    <nav class="view-tabs" aria-label="Dashboard views">
      <button
        v-for="tab in tabs"
        v-show="tab.enabled"
        :key="tab.key"
        type="button"
        class="tab-button"
        :class="{ active: activeTab === tab.key }"
        @click="emit('select', tab.key)"
      >
        {{ tab.label }}
      </button>
    </nav>

    <slot />
  </main>
</template>
```

- [ ] **Step 8: Run frontend typecheck**

Run:

```sh
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

Expected: PASS. `App.vue`, `AppShell.vue`, and `telegram.ts` agree on `DashboardDisplayMode`.

- [ ] **Step 9: Run targeted frontend tests**

Run:

```sh
devenv shell -- npm run test --prefix crates/right-dashboard/frontend
```

Expected: PASS.

- [ ] **Step 10: Commit Task 3**

Run:

```sh
devenv shell -- git add crates/right-dashboard/frontend/src/App.vue crates/right-dashboard/frontend/src/components/AppShell.vue
devenv shell -- git commit -m "feat(dashboard): add display mode toggle"
```

Expected: commit succeeds.

---

### Task 4: Adaptive Normal-Mode Navigation

**Files:**
- Modify: `crates/right-dashboard/frontend/src/App.vue`
- Generated after build: `crates/right-dashboard/static/dashboard/`

- [ ] **Step 1: Add topbar action and display-mode button CSS**

In `crates/right-dashboard/frontend/src/App.vue`, after the existing `.topbar` rule, add:

```css
.topbar-actions {
  display: inline-flex;
  gap: 7px;
  align-items: center;
  justify-content: flex-end;
  min-width: 0;
}

.display-mode-button {
  min-height: 32px;
  padding: 5px 9px;
  border: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
  border-radius: 7px;
  background: var(--tg-theme-secondary-bg-color, #ffffff);
  color: var(--tg-theme-button_color, #2481cc);
  cursor: pointer;
  font-size: 0.78rem;
  font-weight: 700;
  white-space: nowrap;
}
```

- [ ] **Step 2: Keep top navigation horizontally usable on wider layouts**

In `crates/right-dashboard/frontend/src/App.vue`, replace:

```css
.view-tabs,
.subtabs,
.segmented {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 10px;
}
```

with:

```css
.view-tabs,
.subtabs,
.segmented {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 10px;
}

.view-tabs {
  overflow-x: auto;
}
```

- [ ] **Step 3: Add bottom nav only for normal narrow mode**

In `crates/right-dashboard/frontend/src/App.vue`, inside the existing `@media (max-width: 560px)` block, after:

```css
  .meta-grid,
  .meta-grid.compact,
  .model-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
```

add:

```css

  .app-shell.display-normal {
    padding-bottom: calc(74px + env(safe-area-inset-bottom));
  }

  .app-shell.display-normal .view-tabs {
    position: fixed;
    right: 0;
    bottom: 0;
    left: 0;
    z-index: 20;
    flex-wrap: nowrap;
    gap: 6px;
    margin: 0;
    padding: 8px 10px calc(8px + env(safe-area-inset-bottom));
    overflow-x: auto;
    border-top: 1px solid var(--tg-theme-section_separator_color, rgba(84, 102, 117, 0.18));
    background: var(--tg-theme-secondary-bg-color, #ffffff);
    box-shadow: 0 -6px 18px rgba(23, 33, 43, 0.08);
  }

  .app-shell.display-normal .view-tabs .tab-button {
    flex: 0 0 auto;
    min-width: 74px;
  }
```

This keeps fullscreen narrow layouts on the normal top-tab behavior because the selector is scoped to `.display-normal`.

- [ ] **Step 4: Verify frontend typecheck and tests**

Run:

```sh
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run test --prefix crates/right-dashboard/frontend
```

Expected: both commands PASS.

- [ ] **Step 5: Build the frontend and regenerate dashboard assets**

Run:

```sh
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: PASS and `crates/right-dashboard/static/dashboard/` contains regenerated Vite output.

- [ ] **Step 6: Manually inspect the dashboard layout in a browser**

Run:

```sh
devenv shell -- npm run dev --prefix crates/right-dashboard/frontend -- --host 0.0.0.0
```

Expected: Vite prints a local URL. Open it, resize to a narrow mobile-like width, and confirm:

- default mode shows the display-mode button labeled `Fullscreen`;
- default mode does not call fullscreen before user action;
- top-level tabs are at the bottom and scroll horizontally;
- there is no `More` tab;
- bottom content is not hidden under the nav;
- after pressing `Fullscreen`, the preference is saved and the UI switches to fullscreen mode if Telegram APIs are available;
- after pressing `Normal`, the preference is saved as normal.

Stop the dev server with Ctrl-C after inspection.

- [ ] **Step 7: Commit Task 4**

Run:

```sh
devenv shell -- git add crates/right-dashboard/frontend/src/App.vue crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "feat(dashboard): compact normal mode navigation"
```

Expected: commit succeeds. If the build produces no static asset diff, commit only `App.vue`.

---

### Task 5: Final Verification

**Files:**
- Check: full workspace.

- [ ] **Step 1: Run frontend verification**

Run:

```sh
devenv shell -- npm run test --prefix crates/right-dashboard/frontend
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: all PASS.

- [ ] **Step 2: Run mandatory full workspace tests**

Run:

```sh
devenv shell -- cargo test --workspace
```

Expected: PASS.

- [ ] **Step 3: Run mandatory final workspace build**

Run:

```sh
devenv shell -- cargo build --workspace
```

Expected: PASS.

- [ ] **Step 4: Confirm architecture docs did not drift**

Run:

```sh
devenv shell -- sed -n '78,94p' docs/architecture/modules.md
devenv shell -- git diff -- docs/architecture/modules.md ARCHITECTURE.md
```

Expected: no architecture doc diff is required because this feature changes only existing Vue/Vite frontend behavior and generated static assets within the already documented dashboard boundary.

- [ ] **Step 5: Confirm final git status**

Run:

```sh
devenv shell -- git status --short
```

Expected: only intentional committed changes remain. If `npm run build` regenerated assets after the last commit, commit those assets with:

```sh
devenv shell -- git add crates/right-dashboard/static/dashboard
devenv shell -- git commit -m "build(dashboard): regenerate static assets"
```

---

## Self-Review Notes

- Spec coverage: Task 2 covers normal default, saved fullscreen initialization, localStorage persistence, optional Telegram APIs, synchronous failures, and fullscreen event subscriptions. Task 3 covers App/AppShell state flow and toggle behavior. Task 4 covers bottom horizontally scrollable normal-mode navigation and generated assets. Task 5 covers mandatory final workspace verification.
- Scope: no backend API, database, launch-link mode, server-side preference, popup sizing, or `More` tab is introduced.
- Type consistency: the single exported type is `DashboardDisplayMode`; allowed values are `normal` and `fullscreen`; the storage key is `right-dashboard.display-mode`.
