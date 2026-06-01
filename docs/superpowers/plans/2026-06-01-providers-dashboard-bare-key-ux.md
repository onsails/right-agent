# Providers Dashboard "bare key" UX — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the provider dashboard explicit that the `Credential` field holds the bare key (no `Bearer `/scheme prefix), via field microcopy plus a soft, non-blocking warning when a value looks prefixed.

**Architecture:** Frontend only. All decision logic lives in pure functions in `providersViewModel.ts` (unit-tested directly with vitest, the established pattern — see `App.test.ts`/`dashboardTabs.ts`). `ProvidersView.vue` stays a thin consumer: it renders exported hint constants and calls the pure submit-evaluator, so no DOM/interactive test harness is needed (the project has neither jsdom nor @vue/test-utils).

**Tech Stack:** Vue 3 `<script setup>`, TypeScript, vitest (node env), `@vue/server-renderer` for SSR smoke. Files under `crates/right-dashboard/frontend/`.

---

## Pre-flight

Spec: `docs/superpowers/specs/2026-06-01-providers-dashboard-bare-key-ux-design.md`. Convention A ("bare key only") + assist level 2 (microcopy + soft warning) are already approved.

This runs in the dedicated worktree `.claude/worktrees/providers-bare-key-ux`. Land via fast-forward push to `origin/master` per the checkout-churn convention; do not switch `master` in the shared checkout.

All commands run from the worktree root unless stated. The frontend has its own npm package; node_modules is not in git, so install once at baseline.

Baseline verification (record pre-existing failures):

```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npm install && npm test'
```

Expected: PASS (existing dashboard unit/SSR tests are green on `master`). If `npm install` is slow on first run, that is normal; subsequent task steps reuse the installed `node_modules`.

## File Structure

- **Modify:** `crates/right-dashboard/frontend/src/views/providersViewModel.ts` — add `detectCredentialPrefix`, `evaluateCredentialSubmit`, and the exported microcopy constants `CREDENTIAL_HINT` / `HEADER_NAME_HINT`. This file currently holds only `validateSlug`/`validateEnvVar`; it is the natural home for provider-form pure logic.
- **Create:** `crates/right-dashboard/frontend/src/views/providersViewModel.test.ts` — unit tests for the new pure functions.
- **Modify:** `crates/right-dashboard/frontend/src/views/ProvidersView.vue` — render hints, wire the soft warning into `submitAdd`/`submitRotate`, re-arm on value change, reset on modal open/close, add `.hint`/`.notice.warn` styles.
- **Create:** `crates/right-dashboard/frontend/src/views/ProvidersView.test.ts` — SSR smoke test (component renders without throwing).

No backend, gateway, or `agent.yaml` changes (the existing config is correct; this is a UX fix).

---

### Task 1: Pure logic + microcopy in `providersViewModel.ts`

**Files:**
- Create: `crates/right-dashboard/frontend/src/views/providersViewModel.test.ts`
- Modify: `crates/right-dashboard/frontend/src/views/providersViewModel.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/views/providersViewModel.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import {
  detectCredentialPrefix,
  evaluateCredentialSubmit,
  CREDENTIAL_HINT,
  HEADER_NAME_HINT,
} from './providersViewModel'

describe('detectCredentialPrefix', () => {
  it('flags known auth-scheme prefixes (case-insensitive, returns canonical case)', () => {
    expect(detectCredentialPrefix('Bearer abc123')).toBe('Bearer')
    expect(detectCredentialPrefix('bearer abc123')).toBe('Bearer')
    expect(detectCredentialPrefix('  Basic dXNlcjpwYXNz')).toBe('Basic')
    expect(detectCredentialPrefix('Token gho_xxx')).toBe('Token')
    expect(detectCredentialPrefix('Bot 123:ABC')).toBe('Bot')
    expect(detectCredentialPrefix('Digest xyz')).toBe('Digest')
  })

  it('does not flag a bare key', () => {
    expect(detectCredentialPrefix('sk-abc123')).toBeNull()
    expect(detectCredentialPrefix('gho_1234567890')).toBeNull()
    expect(detectCredentialPrefix('')).toBeNull()
  })

  it('does not flag substrings or scheme-word without a trailing value', () => {
    expect(detectCredentialPrefix('bearertoken-no-space')).toBeNull()
    expect(detectCredentialPrefix('my-bearer-key')).toBeNull()
    expect(detectCredentialPrefix('Bearer')).toBeNull()
    expect(detectCredentialPrefix('Bearer   ')).toBeNull()
  })
})

describe('evaluateCredentialSubmit', () => {
  it('proceeds for a bare key', () => {
    expect(evaluateCredentialSubmit('sk-abc', false)).toEqual({ proceed: true, warning: null })
  })

  it('blocks the first time a prefixed value is submitted and names the scheme', () => {
    const r = evaluateCredentialSubmit('Bearer sk-abc', false)
    expect(r.proceed).toBe(false)
    expect(r.warning).toContain('Bearer')
    expect(r.warning).toContain('bare key')
  })

  it('proceeds on the second submit (already warned) of the same prefixed value', () => {
    expect(evaluateCredentialSubmit('Bearer sk-abc', true)).toEqual({ proceed: true, warning: null })
  })
})

describe('microcopy', () => {
  it('credential hint tells users to omit Bearer/header', () => {
    expect(CREDENTIAL_HINT).toContain('Bearer')
    expect(CREDENTIAL_HINT.toLowerCase()).toContain('only')
  })

  it('header-name hint explains the consumer must match it', () => {
    expect(HEADER_NAME_HINT).toContain('Authorization')
  })
})
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npx vitest run src/views/providersViewModel.test.ts'
```
Expected: FAIL — `detectCredentialPrefix`, `evaluateCredentialSubmit`, `CREDENTIAL_HINT`, `HEADER_NAME_HINT` are not exported.

- [ ] **Step 3: Implement the pure functions and constants**

Append to `crates/right-dashboard/frontend/src/views/providersViewModel.ts` (keep existing `validateSlug`/`validateEnvVar` untouched):

```ts
/** Known HTTP auth-scheme prefixes a user might wrongly paste into the key. */
const CREDENTIAL_SCHEME_PREFIXES = ['Bearer', 'Basic', 'Token', 'Bot', 'Digest'] as const

/**
 * Returns the canonical-cased auth-scheme prefix the value appears to start
 * with (e.g. "Bearer"), or null. Match is case-insensitive on the first
 * whitespace-delimited word and requires a following space + more text, so a
 * key that merely contains "bearer" as a substring is NOT flagged.
 */
export function detectCredentialPrefix(value: string): string | null {
  const trimmed = value.trimStart()
  const space = trimmed.indexOf(' ')
  if (space <= 0) return null
  const firstWord = trimmed.slice(0, space)
  const rest = trimmed.slice(space + 1).trim()
  if (rest.length === 0) return null
  return (
    CREDENTIAL_SCHEME_PREFIXES.find((p) => p.toLowerCase() === firstWord.toLowerCase()) ?? null
  )
}

/**
 * Decide whether a credential submit should proceed. The first time a
 * prefixed value is seen (`alreadyWarned === false`) it blocks and returns a
 * warning; once the user has been warned, the same value proceeds.
 */
export function evaluateCredentialSubmit(
  value: string,
  alreadyWarned: boolean,
): { proceed: boolean; warning: string | null } {
  const prefix = detectCredentialPrefix(value)
  if (prefix && !alreadyWarned) {
    return {
      proceed: false,
      warning: `This looks like it includes a "${prefix}" prefix. Providers store the bare key — the consumer adds it. Remove it, or press Save again to keep as-is.`,
    }
  }
  return { proceed: true, warning: null }
}

/** Microcopy shown under the Credential field (add/rotate). */
export const CREDENTIAL_HINT =
  'Paste only the key/token itself — no "Bearer ", no header name. The skill or agent adds any prefix.'

/** Microcopy shown under the Header name field (add/edit generic). */
export const HEADER_NAME_HINT =
  'HTTP header the skill/agent puts the key into for requests to the upstream host. Must match what the consumer sends (e.g. Authorization for Typefully).'
```

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npx vitest run src/views/providersViewModel.test.ts'
```
Expected: PASS (all cases).

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/providersViewModel.ts crates/right-dashboard/frontend/src/views/providersViewModel.test.ts
PREK_ALLOW_NO_CONFIG=1 git commit -m "feat(dashboard): bare-key credential detector + microcopy helpers"
```

---

### Task 2: Wire microcopy + soft warning into `ProvidersView.vue`

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/ProvidersView.vue`

Reference points in the current file: imports at line 1-17; refs `addCredential` (30), `addHeaderName` (33), `addError` (37), `rotateCredential` (46), `rotateError` (48); `openAdd` (89), `closeAdd` (112), `submitAdd` (138, credential-required check at 152), `openRotate` (177), `closeRotate` (184), `submitRotate` (191, credential-required check at 193); template generic fields (344-366), rotate modal (380-393), edit-generic modal (396-423).

- [ ] **Step 1: Import vue `watch` and the new helpers**

Change the top imports of `ProvidersView.vue`.

From:
```ts
import { onBeforeUnmount, onMounted, ref } from 'vue'
```
To:
```ts
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
```

And extend the `providersViewModel` import (currently `import { validateSlug, validateEnvVar } from './providersViewModel'`) to:
```ts
import {
  validateSlug,
  validateEnvVar,
  evaluateCredentialSubmit,
  CREDENTIAL_HINT,
  HEADER_NAME_HINT,
} from './providersViewModel'
```

- [ ] **Step 2: Add warning + ack state**

Immediately after the `const addError = ref<string | null>(null)` line (line 37) add:
```ts
const addWarn = ref<string | null>(null)
```
Immediately after `const rotateError = ref<string | null>(null)` (line 48) add:
```ts
const rotateWarn = ref<string | null>(null)
// Set once a prefixed credential has been flagged; a second Save then proceeds.
const credentialWarnAck = ref(false)
```

Add re-arm watchers near the end of `<script setup>` (before `</script>`, after the existing helper functions):
```ts
// Editing the credential re-arms the soft prefix warning.
watch(addCredential, () => {
  credentialWarnAck.value = false
  addWarn.value = null
})
watch(rotateCredential, () => {
  credentialWarnAck.value = false
  rotateWarn.value = null
})
```

- [ ] **Step 3: Reset warning state on open/close**

In `openAdd` (after `addError.value = null`, ~line 99) and `closeAdd` (after `addError.value = null`, ~line 122) add:
```ts
  addWarn.value = null
  credentialWarnAck.value = false
```
In `openRotate` (after `rotateError.value = null`, ~line 180) and `closeRotate` (after `rotateError.value = null`, ~line 188) add:
```ts
  rotateWarn.value = null
  credentialWarnAck.value = false
```

- [ ] **Step 4: Gate `submitAdd` on the soft warning**

In `submitAdd`, immediately AFTER the credential-required check (`if (!addCredential.value.trim()) { addError.value = 'Credential is required'; return }`, line 152) and BEFORE `addBusy.value = true` (line 154), insert:
```ts
    const credCheck = evaluateCredentialSubmit(addCredential.value, credentialWarnAck.value)
    if (!credCheck.proceed) {
      addWarn.value = credCheck.warning
      credentialWarnAck.value = true
      return
    }
    addWarn.value = null
```

- [ ] **Step 5: Gate `submitRotate` on the soft warning**

In `submitRotate`, immediately AFTER the credential-required check (`if (!rotateCredential.value.trim()) { rotateError.value = 'Credential is required'; return }`, line 193) and BEFORE `rotateBusy.value = true` (line 194), insert:
```ts
  const credCheck = evaluateCredentialSubmit(rotateCredential.value, credentialWarnAck.value)
  if (!credCheck.proceed) {
    rotateWarn.value = credCheck.warning
    credentialWarnAck.value = true
    return
  }
  rotateWarn.value = null
```

- [ ] **Step 6: Render the header-name hint (add-generic)**

In the generic add form, replace the Header name field block (lines 353-356):
```html
          <label class="field">
            <span class="label">Header name (optional)</span>
            <input v-model="addHeaderName" class="text-input" autocomplete="off" placeholder="e.g. Authorization">
          </label>
```
with:
```html
          <label class="field">
            <span class="label">Header name (optional)</span>
            <input v-model="addHeaderName" class="text-input" autocomplete="off" placeholder="e.g. Authorization">
            <span class="hint">{{ HEADER_NAME_HINT }}</span>
          </label>
```

- [ ] **Step 7: Render the credential hint (add form)**

Replace the credential field block (lines 363-366):
```html
        <label class="field full-width">
          <span class="label">Credential (API key)</span>
          <SecretInput v-model="addCredential" placeholder="Paste API key" />
        </label>
```
with:
```html
        <label class="field full-width">
          <span class="label">Credential (API key)</span>
          <SecretInput v-model="addCredential" placeholder="Paste API key" />
          <span class="hint">{{ CREDENTIAL_HINT }}</span>
        </label>
```

- [ ] **Step 8: Render the add warning notice**

Directly after the add-form error line (`<p v-if="addError" class="notice inline">{{ addError }}</p>`, line 369) add:
```html
      <p v-if="addWarn" class="notice inline warn">{{ addWarn }}</p>
```

- [ ] **Step 9: Render credential hint + warning in the Rotate modal**

In the rotate modal, replace the credential field + error (lines 382-386):
```html
      <label class="field">
        <span class="label">New credential</span>
        <SecretInput v-model="rotateCredential" placeholder="Paste new API key" />
      </label>
      <p v-if="rotateError" class="notice inline">{{ rotateError }}</p>
```
with:
```html
      <label class="field">
        <span class="label">New credential</span>
        <SecretInput v-model="rotateCredential" placeholder="Paste new API key" />
        <span class="hint">{{ CREDENTIAL_HINT }}</span>
      </label>
      <p v-if="rotateError" class="notice inline">{{ rotateError }}</p>
      <p v-if="rotateWarn" class="notice inline warn">{{ rotateWarn }}</p>
```

- [ ] **Step 10: Render the header-name hint in the Edit (generic) modal**

In the edit modal, replace the Header name field block (lines 407-410):
```html
        <label class="field">
          <span class="label">Header name (optional)</span>
          <input v-model="editHeaderName" class="text-input" autocomplete="off">
        </label>
```
with:
```html
        <label class="field">
          <span class="label">Header name (optional)</span>
          <input v-model="editHeaderName" class="text-input" autocomplete="off">
          <span class="hint">{{ HEADER_NAME_HINT }}</span>
        </label>
```

- [ ] **Step 11: Add `.hint` and `.notice.warn` styles**

In the `<style scoped>` block, after the `.label { ... }` rule (ends ~line 517) add:
```css
.hint {
  color: var(--tg-theme-hint-color, #6b7b88);
  font-size: 0.72rem;
  line-height: 1.35;
}

.notice.warn {
  color: var(--tg-theme-text-color, #17212b);
  background: rgba(214, 165, 26, 0.14);
  border-radius: 7px;
  padding: 6px 8px;
}
```

- [ ] **Step 12: Typecheck**

Run:
```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npm run typecheck'
```
Expected: PASS (no `vue-tsc` errors). Common failure: forgetting to add `watch` to the vue import, or a template referencing an unimported constant.

- [ ] **Step 13: Add an SSR smoke test for the component**

Create `crates/right-dashboard/frontend/src/views/ProvidersView.test.ts`:

```ts
import { describe, expect, it, vi } from 'vitest'
import { renderToString } from '@vue/server-renderer'
import { createApp } from 'vue'

// The view calls these on mount; stub them so SSR doesn't hit the network.
vi.mock('../api', () => ({
  providerList: () => Promise.resolve({ providers: [] }),
  providerTypes: () => Promise.resolve({ types: [] }),
  providerCreate: () => Promise.resolve({}),
  providerRotate: () => Promise.resolve({}),
  providerConfigUpdate: () => Promise.resolve({}),
  providerRemove: () => Promise.resolve({}),
}))

import ProvidersView from './ProvidersView.vue'

describe('ProvidersView', () => {
  it('renders the panel without throwing', async () => {
    const html = await renderToString(createApp(ProvidersView))
    expect(html).toContain('Providers')
  })
})
```

- [ ] **Step 14: Run the component test**

Run:
```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npx vitest run src/views/ProvidersView.test.ts'
```
Expected: PASS. (SSR renders the closed-modal initial state; the panel heading "Providers" is present. The hint/warning interactive paths are covered by Task 1's pure-function unit tests, since the project has no jsdom/@vue/test-utils harness for click simulation — note this limitation rather than adding a heavy dependency.)

- [ ] **Step 15: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/ProvidersView.vue crates/right-dashboard/frontend/src/views/ProvidersView.test.ts
PREK_ALLOW_NO_CONFIG=1 git commit -m "feat(dashboard): bare-key hints + soft prefix warning in providers form"
```

---

### Task 3: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Full frontend test suite**

Run:
```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npm test'
```
Expected: PASS for all tests including the two new files. Record any pre-existing flakes seen at baseline.

- [ ] **Step 2: Typecheck + production build**

Run:
```bash
devenv shell -- bash -lc 'cd crates/right-dashboard/frontend && npm run build'
```
Expected: `vue-tsc --noEmit` passes and `vite build` succeeds.

- [ ] **Step 3: Full workspace test (mandatory per AGENTS.md)**

Run:
```bash
devenv shell -- cargo test --workspace
```
Expected: PASS. No Rust changed, so this should match the worktree baseline; the final workspace test is mandatory regardless. Known potential flakes under parallel load: cc/invocation pid race and dashboard warn-count tests — re-run isolated before attributing to this change.

---

## Out of scope (with rationale)

- **Backend / gateway / `agent.yaml` changes.** The provider config is mechanically correct; the failure was a user-entered `Bearer ` prefix in the value. This is a UX clarity fix only.
- **Fixing the live `right-typefully` provider.** Re-entering the bare key via the dashboard Rotate action is a separate user action (the new UI guides it). Not code.
- **Live final-header preview.** The dashboard can't know whether the skill prepends `Bearer`; a preview would mislead.
- **Hard-block on detected prefix.** Rejected in design (assist level 3) — annoying on false positives, and the control plane should not override the user.
- **i18n / Russian translation.** The dashboard UI is English; microcopy stays English.

## Self-Review

- **Spec coverage:** Convention A "bare key" → microcopy constants (Task 1) rendered everywhere a credential/header is entered (Task 2 steps 6-10). Assist level 2 soft warning → `evaluateCredentialSubmit` + ack flow + re-arm + reset (Task 1; Task 2 steps 2-5). Scope table (add-generic, add-built-in, rotate, edit-generic) → submitAdd covers both add types; rotate covered; edit gets the header hint only (no credential field there). Non-goals respected (no backend, no preview, no hard block, English).
- **Placeholder scan:** No TBD/TODO; every code step shows complete code; every command has an expected result.
- **Type consistency:** `detectCredentialPrefix(value): string | null`, `evaluateCredentialSubmit(value, alreadyWarned): { proceed, warning }`, `CREDENTIAL_HINT`/`HEADER_NAME_HINT` strings — used identically in tests (Task 1) and the component (Task 2). `addWarn`/`rotateWarn`/`credentialWarnAck` ref names consistent across steps 2-5 and the template (steps 8-9).
