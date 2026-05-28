# Dashboard Polishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Polish the Telegram Mini App dashboard — reorder Overview, fix signal-detail browsing, delete legacy usage rows, make Identity legible (and fix the sandbox-read timeout behind it), collapse Skills groups, and kill the loading-flash with shared components.

**Architecture:** Three new shared Vue components (`Spinner`, `AsyncState`, `CollapsibleSection`) are built first and consumed by every view. One additive `right-db` migration deletes dead usage rows. The Identity backend coalesces three sequential sandbox reads into one and distinguishes "timed out" from "absent". All decision logic is extracted into pure helpers tested with vitest; components are smoke-tested with Vue SSR `renderToString` (the existing pattern — there is **no** `@vue/test-utils`).

**Tech Stack:** Rust (edition 2024, `right-db` Turso migrations, bot dashboard handlers), Vue 3 + Vite + TypeScript, vitest, `@vue/server-renderer`.

---

## Conventions for every task

- Run all commands from the repo root `/Users/molt/dev/rightclaw`.
- Rust commands are prefixed `devenv shell --`. Frontend commands run inside `crates/right-dashboard/frontend`.
- Frontend test runner: `cd crates/right-dashboard/frontend && pnpm test` (vitest, runs all). Single file: `pnpm test -- src/path/to/file.test.ts`. Type-check: `pnpm typecheck`.
- Component tests use Vue SSR (`createSSRApp` + `renderToString` from `@vue/server-renderer`) — copy the shape of `src/components/AppShell.test.ts`. Pure-logic tests import a `*.ts` helper directly — copy the shape of `src/components/secretInputModel.ts` + `SecretInput.test.ts`.
- Commit after every task with a Conventional Commit message.

---

## File Structure

**New files**
- `crates/right-db/src/sql/v37_drop_legacy_usage_sources.sql` — data migration.
- `crates/right-dashboard/frontend/src/components/Spinner.vue` — spinner atom.
- `crates/right-dashboard/frontend/src/components/asyncState.ts` — pure state resolver.
- `crates/right-dashboard/frontend/src/components/AsyncState.vue` — loading/empty/error wrapper.
- `crates/right-dashboard/frontend/src/components/AsyncState.test.ts` — resolver + SSR tests.
- `crates/right-dashboard/frontend/src/components/CollapsibleSection.vue` — collapsible group.
- `crates/right-dashboard/frontend/src/components/CollapsibleSection.test.ts` — SSR test.
- `crates/right-dashboard/frontend/src/components/identityLabels.ts` — identity state → label/tone.
- `crates/right-dashboard/frontend/src/components/identityLabels.test.ts` — unit tests.
- `crates/bot/src/telegram/dashboard/identity_parse.rs` — pure combined-read parser + state mapping.

**Modified files**
- `crates/right-db/src/migrations.rs` — register v37, bump `LATEST_SCHEMA_VERSION`.
- `crates/bot/src/telegram/dashboard/identity.rs` — single coalesced read, timeout maps to `sandbox_unreachable`.
- `crates/right-dashboard/frontend/src/views/IdentityView.vue` — dedup, labels, banner, retry, AsyncState.
- `crates/right-dashboard/frontend/src/views/SkillsView.vue` — collapsible groups w/ counts.
- `crates/right-dashboard/frontend/src/views/OverviewView.vue` — reorder + inline accordion.
- `crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue` — expandable rows.
- `crates/right-dashboard/frontend/src/views/UsageView.vue`, `HealthView.vue`, `ActivityView.vue` — AsyncState.
- `crates/right-dashboard/frontend/src/App.vue` — pass loading/error to Overview & Usage; identity retry handler.
- `crates/right-dashboard/frontend/src/types.ts` — identity refresh emit (if typecheck requires).
- `ARCHITECTURE.md` — Dashboard frontend primitives rule.

---

## Task 1: Delete legacy usage rows (v37 migration)

**Files:**
- Create: `crates/right-db/src/sql/v37_drop_legacy_usage_sources.sql`
- Modify: `crates/right-db/src/migrations.rs` (const block ~line 32, `LATEST_SCHEMA_VERSION` line 34, end of `MIGRATIONS` array)
- Test: `crates/right-db/src/migrations.rs` (`#[cfg(test)]` module, alongside existing migration tests)

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/right-db/src/migrations.rs` (follow the existing `MIGRATIONS.to_latest(&mut conn)` test style):

```rust
#[tokio::test]
async fn v37_deletes_legacy_learning_usage_sources() {
    let conn = crate::test_support::memory_connection().await;
    // Bring schema up to just before v37 so usage_events exists (v15).
    MIGRATIONS.to_version(&conn, 36).await.unwrap();
    conn.execute_batch(
        "INSERT INTO usage_events (session_uuid, source, ts, total_cost_usd, num_turns, \
         input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens, \
         service_tier_breakdown, model_breakdown, delivery) VALUES \
         ('s1','learning_reviewer','2026-01-01T00:00:00Z',0.10,1,0,0,0,0,'{}','{}','none'), \
         ('s2','learning_selector','2026-01-01T00:00:00Z',0.20,1,0,0,0,0,'{}','{}','none'), \
         ('s3','interactive','2026-01-01T00:00:00Z',0.30,1,0,0,0,0,'{}','{}','none');",
    )
    .await
    .unwrap();

    MIGRATIONS.to_latest(&conn).await.unwrap();

    let legacy = conn
        .query_i64(
            "SELECT COUNT(*) FROM usage_events WHERE source IN ('learning_reviewer','learning_selector')",
            crate::migrations::MigrationParams::Empty,
        )
        .await
        .unwrap();
    assert_eq!(legacy, 0, "legacy learning usage rows must be deleted");

    let kept = conn
        .query_i64(
            "SELECT COUNT(*) FROM usage_events WHERE source = 'interactive'",
            crate::migrations::MigrationParams::Empty,
        )
        .await
        .unwrap();
    assert_eq!(kept, 1, "non-legacy rows must be preserved");

    // Idempotent: re-running to_latest is a no-op and does not error.
    MIGRATIONS.to_latest(&conn).await.unwrap();
}
```

> Note: confirm the exact `usage_events` column list against `crates/right-db/src/sql/v15_usage_events.sql` before running; adjust the INSERT to match. Confirm the test-support memory-connection helper name with `rg -n "memory_connection|fn .*memory.*Connection" crates/right-db/src`. If `query_i64` is not `pub`, use a `SELECT` through the public query API used by neighbouring tests.

- [ ] **Step 2: Run test to verify it fails**

Run: `devenv shell -- cargo test -p right-db v37_deletes_legacy_learning_usage_sources`
Expected: FAIL — either a compile error referencing the missing migration/version, or the assert (`legacy == 0`) fails because no v37 exists yet.

- [ ] **Step 3: Create the migration SQL**

Create `crates/right-db/src/sql/v37_drop_legacy_usage_sources.sql`:

```sql
-- Remove usage rows from a retired learning pipeline (learning_reviewer,
-- learning_selector). No current code writes these sources; they are dead
-- residue surfaced as "unknown usage source" dashboard warnings. DELETE is
-- idempotent — a re-run removes zero rows.
DELETE FROM usage_events WHERE source IN ('learning_reviewer', 'learning_selector');
```

- [ ] **Step 4: Register the migration**

In `crates/right-db/src/migrations.rs`:

Add the const after the v36 line (~line 32):

```rust
const V37_SCHEMA: &str = include_str!("sql/v37_drop_legacy_usage_sources.sql");
```

Bump the version constant (line 34):

```rust
pub const LATEST_SCHEMA_VERSION: u32 = 37;
```

Append to the end of the `MIGRATIONS.migrations` array (after the `version: 36` entry):

```rust
        Migration {
            version: 37,
            sql: V37_SCHEMA,
            hook: None,
        },
```

- [ ] **Step 5: Run test to verify it passes**

Run: `devenv shell -- cargo test -p right-db v37_deletes_legacy_learning_usage_sources`
Expected: PASS.

- [ ] **Step 6: Run the full right-db migration suite (no regressions)**

Run: `devenv shell -- cargo test -p right-db`
Expected: PASS (including the existing `migration_runner_semantics_*` and idempotency tests).

- [ ] **Step 7: Commit**

```bash
git add crates/right-db/src/sql/v37_drop_legacy_usage_sources.sql crates/right-db/src/migrations.rs
git commit -m "feat(right-db): drop legacy learning_reviewer/learning_selector usage rows (v37)"
```

---

## Task 2: `AsyncState` resolver + component

The pure resolver is the testable core; the `.vue` is a thin renderer.

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/asyncState.ts`
- Create: `crates/right-dashboard/frontend/src/components/Spinner.vue`
- Create: `crates/right-dashboard/frontend/src/components/AsyncState.vue`
- Test: `crates/right-dashboard/frontend/src/components/AsyncState.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/AsyncState.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import AsyncState from './AsyncState.vue'
import { resolveAsyncState } from './asyncState'

describe('resolveAsyncState', () => {
  it('prioritises error over everything', () => {
    expect(resolveAsyncState({ loading: true, error: 'boom', empty: true })).toBe('error')
  })
  it('shows loading when no error and still loading', () => {
    expect(resolveAsyncState({ loading: true, error: null, empty: true })).toBe('loading')
  })
  it('shows empty when loaded but empty', () => {
    expect(resolveAsyncState({ loading: false, error: null, empty: true })).toBe('empty')
  })
  it('shows content when loaded and non-empty', () => {
    expect(resolveAsyncState({ loading: false, error: null, empty: false })).toBe('content')
  })
})

describe('AsyncState component', () => {
  async function render(props: Record<string, unknown>) {
    const app = createSSRApp({
      render: () => h(AsyncState, props, () => h('p', 'CONTENT')),
    })
    return renderToString(app)
  }
  it('renders the slot when content state', async () => {
    expect(await render({ loading: false, error: null, empty: false })).toContain('CONTENT')
  })
  it('renders the error text on error', async () => {
    const html = await render({ loading: false, error: 'nope', empty: false })
    expect(html).toContain('nope')
    expect(html).not.toContain('CONTENT')
  })
  it('renders a spinner while loading', async () => {
    const html = await render({ loading: true, error: null, empty: true })
    expect(html).toContain('spinner')
    expect(html).not.toContain('CONTENT')
  })
  it('renders emptyText when empty', async () => {
    const html = await render({ loading: false, error: null, empty: true, emptyText: 'Nothing here' })
    expect(html).toContain('Nothing here')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm test -- src/components/AsyncState.test.ts`
Expected: FAIL — modules `./asyncState` and `./AsyncState.vue` do not exist.

- [ ] **Step 3: Implement the resolver**

Create `crates/right-dashboard/frontend/src/components/asyncState.ts`:

```ts
export type AsyncStateKind = 'error' | 'loading' | 'empty' | 'content'

export interface AsyncStateInput {
  loading: boolean
  error: string | null
  empty: boolean
}

/**
 * Decide what an async panel should render. Error wins over loading so a
 * failed refresh is never masked by a spinner; loading wins over empty so a
 * not-yet-loaded panel never flashes "nothing here".
 */
export function resolveAsyncState(input: AsyncStateInput): AsyncStateKind {
  if (input.error) {
    return 'error'
  }
  if (input.loading) {
    return 'loading'
  }
  if (input.empty) {
    return 'empty'
  }
  return 'content'
}
```

- [ ] **Step 4: Implement the Spinner atom**

Create `crates/right-dashboard/frontend/src/components/Spinner.vue`:

```vue
<template>
  <span class="spinner" role="status" aria-label="Loading" />
</template>

<style scoped>
.spinner {
  display: inline-block;
  width: 18px;
  height: 18px;
  border: 2px solid var(--tg-theme-hint_color, #888);
  border-top-color: transparent;
  border-radius: 50%;
  animation: spinner-rotate 0.7s linear infinite;
}
@keyframes spinner-rotate {
  to { transform: rotate(360deg); }
}
@media (prefers-reduced-motion: reduce) {
  .spinner { animation-duration: 2s; }
}
</style>
```

- [ ] **Step 5: Implement the AsyncState wrapper**

Create `crates/right-dashboard/frontend/src/components/AsyncState.vue`:

```vue
<script setup lang="ts">
import { computed } from 'vue'
import Spinner from './Spinner.vue'
import { resolveAsyncState } from './asyncState'

const props = withDefaults(defineProps<{
  loading: boolean
  error: string | null
  empty: boolean
  emptyText?: string
}>(), { emptyText: 'No data' })

const kind = computed(() => resolveAsyncState({
  loading: props.loading,
  error: props.error,
  empty: props.empty,
}))
</script>

<template>
  <p v-if="kind === 'error'" class="notice inline">{{ error }}</p>
  <div v-else-if="kind === 'loading'" class="async-loading">
    <Spinner />
  </div>
  <p v-else-if="kind === 'empty'" class="muted-line">{{ emptyText }}</p>
  <slot v-else />
</template>

<style scoped>
.async-loading {
  display: flex;
  justify-content: center;
  padding: 24px 0;
}
</style>
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm test -- src/components/AsyncState.test.ts`
Expected: PASS (all resolver + component cases).

- [ ] **Step 7: Type-check**

Run: `cd crates/right-dashboard/frontend && pnpm typecheck`
Expected: no errors.

- [ ] **Step 8: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/asyncState.ts \
  crates/right-dashboard/frontend/src/components/Spinner.vue \
  crates/right-dashboard/frontend/src/components/AsyncState.vue \
  crates/right-dashboard/frontend/src/components/AsyncState.test.ts
git commit -m "feat(dashboard): add Spinner + AsyncState loading/empty/error primitive"
```

---

## Task 3: `CollapsibleSection` component

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/CollapsibleSection.vue`
- Test: `crates/right-dashboard/frontend/src/components/CollapsibleSection.test.ts`

- [ ] **Step 1: Write the failing test**

Create `crates/right-dashboard/frontend/src/components/CollapsibleSection.test.ts`:

```ts
import { renderToString } from '@vue/server-renderer'
import { createSSRApp, h } from 'vue'
import { describe, expect, it } from 'vitest'

import CollapsibleSection from './CollapsibleSection.vue'

async function render(props: Record<string, unknown>) {
  const app = createSSRApp({
    render: () => h(CollapsibleSection, props, () => h('p', 'BODY')),
  })
  return renderToString(app)
}

describe('CollapsibleSection', () => {
  it('shows the title and count badge', async () => {
    const html = await render({ title: 'core', count: 3 })
    expect(html).toContain('core')
    expect(html).toContain('3')
  })
  it('hides the body when collapsed by default', async () => {
    const html = await render({ title: 'core', count: 3 })
    expect(html).not.toContain('BODY')
  })
  it('shows the body when defaultOpen is true', async () => {
    const html = await render({ title: 'core', count: 3, defaultOpen: true })
    expect(html).toContain('BODY')
  })
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm test -- src/components/CollapsibleSection.test.ts`
Expected: FAIL — `./CollapsibleSection.vue` does not exist.

- [ ] **Step 3: Implement the component**

Create `crates/right-dashboard/frontend/src/components/CollapsibleSection.vue`:

```vue
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm test -- src/components/CollapsibleSection.test.ts`
Expected: PASS.

- [ ] **Step 5: Type-check and commit**

Run: `cd crates/right-dashboard/frontend && pnpm typecheck` (expect no errors), then:

```bash
git add crates/right-dashboard/frontend/src/components/CollapsibleSection.vue \
  crates/right-dashboard/frontend/src/components/CollapsibleSection.test.ts
git commit -m "feat(dashboard): add CollapsibleSection with count badge"
```

---

## Task 4: Identity backend — coalesced sandbox read + state machine

Read all three identity files in **one** in-sandbox command, with a length-prefixed framing so content is parsed unambiguously, and map a timeout/error to `sandbox_unreachable` (distinct from `not_authored`/`host_mirror`).

**Files:**
- Create: `crates/bot/src/telegram/dashboard/identity_parse.rs`
- Modify: `crates/bot/src/telegram/dashboard/identity.rs` (the `read_sandbox_identity_files` fn near `:90-150`, the `SANDBOX_READ_IDENTITY_SCRIPT` near `:13-17`, and `host_mirror_or_unavailable` near `:210-221`)
- Test: `crates/bot/src/telegram/dashboard/identity_parse.rs` (`#[cfg(test)]`)

> Background: the in-sandbox runner returns `miette::Result<(String, i32)>` (stdout, exit_code). `IDENTITY_PREVIEW_LIMIT_BYTES = 64 * 1024`. Identity file names: `["IDENTITY.md","SOUL.md","USER.md"]` (`right_dashboard::identity_files::IDENTITY_FILE_NAMES`). The per-file `source` string is what the frontend maps to a label; the agreed states are `sandbox`, `sandbox_unreachable`, `host_mirror`, `not_authored`, `host`, `missing`.

- [ ] **Step 1: Write the parser module with failing unit tests**

Create `crates/bot/src/telegram/dashboard/identity_parse.rs`:

```rust
//! Pure parser for the coalesced sandbox identity read.

/// One file's result from the combined sandbox read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ParsedIdentityRead {
    pub name: String,
    pub present: bool,
    pub content: String,
    /// True when the file was longer than the requested preview limit.
    pub truncated: bool,
}

/// Combined-read framing: each file is emitted as a header line
/// `RIGHT_IDENTITY <name> <PRESENT|ABSENT> <byte_count>\n` followed by exactly
/// `byte_count` content bytes and a trailing `\n`. `preview_limit` is the
/// number of content bytes requested per file (the script asks for
/// `preview_limit + 1` so truncation is detectable).
pub(super) fn parse_combined_identity_read(
    stdout: &str,
    preview_limit: usize,
) -> Vec<ParsedIdentityRead> {
    let mut out = Vec::new();
    let bytes = stdout.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() {
        let Some(nl) = bytes[idx..].iter().position(|&b| b == b'\n') else {
            break;
        };
        let header = &stdout[idx..idx + nl];
        idx += nl + 1;
        let mut parts = header.splitn(4, ' ');
        if parts.next() != Some("RIGHT_IDENTITY") {
            continue;
        }
        let (Some(name), Some(file_state), Some(count)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let n: usize = count.trim().parse().unwrap_or(0);
        if file_state == "ABSENT" {
            out.push(ParsedIdentityRead {
                name: name.to_owned(),
                present: false,
                content: String::new(),
                truncated: false,
            });
            continue;
        }
        let end = (idx + n).min(bytes.len());
        let mut content = stdout[idx..end].to_owned();
        idx = end;
        if idx < bytes.len() && bytes[idx] == b'\n' {
            idx += 1;
        }
        let truncated = n > preview_limit;
        if truncated {
            let cut = content
                .char_indices()
                .take_while(|(i, _)| *i < preview_limit)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            content.truncate(cut);
        }
        out.push(ParsedIdentityRead {
            name: name.to_owned(),
            present: true,
            content,
            truncated,
        });
    }
    out
}

/// Per-file state for a sandboxed agent given sandbox + host-mirror presence.
/// Timeout/exec errors are handled by the caller (mapped to
/// `sandbox_unreachable`); this only covers a successful combined read.
pub(super) fn identity_state(sandbox_present: bool, host_present: bool) -> &'static str {
    match (sandbox_present, host_present) {
        (true, _) => "sandbox",
        (false, true) => "host_mirror",
        (false, false) => "not_authored",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_present_and_absent_files() {
        let stdout = "RIGHT_IDENTITY IDENTITY.md PRESENT 6\n# hi\n\
                      RIGHT_IDENTITY SOUL.md ABSENT 0\n\
                      RIGHT_IDENTITY USER.md PRESENT 4\nyou\n\n";
        let parsed = parse_combined_identity_read(stdout, 64 * 1024);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].name, "IDENTITY.md");
        assert!(parsed[0].present);
        assert_eq!(parsed[0].content, "# hi\n");
        assert!(!parsed[1].present);
        assert_eq!(parsed[2].content, "you\n");
    }

    #[test]
    fn marks_truncated_when_over_limit() {
        let stdout = "RIGHT_IDENTITY IDENTITY.md PRESENT 4\nabcd\n";
        let parsed = parse_combined_identity_read(stdout, 3);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].truncated);
        assert_eq!(parsed[0].content, "abc");
    }

    #[test]
    fn maps_states_from_sandbox_and_host_presence() {
        assert_eq!(identity_state(true, true), "sandbox");
        assert_eq!(identity_state(true, false), "sandbox");
        assert_eq!(identity_state(false, true), "host_mirror");
        assert_eq!(identity_state(false, false), "not_authored");
    }
}
```

- [ ] **Step 2: Declare the submodule and run the failing tests**

Add `mod identity_parse;` where the identity module declares siblings.

> Confirm the declaration site: run `rg -n "mod identity|mod health|mod skills" crates/bot/src/telegram/dashboard.rs crates/bot/src/telegram/dashboard/identity.rs`. Add `mod identity_parse;` next to where the identity module's peers are declared, matching the existing pattern (it will likely be inside `identity.rs` as `mod identity_parse;` if `identity.rs` is itself `mod identity` in `dashboard.rs`).

Run: `devenv shell -- cargo test -p bot identity_parse`
Expected: PASS once it compiles (the parser + mapping are self-contained). If it does not compile, fix the module declaration first.

- [ ] **Step 3: Replace the in-sandbox read script**

Replace `SANDBOX_READ_IDENTITY_SCRIPT` (near `:13-17`) with a combined-read script:

```rust
const SANDBOX_READ_IDENTITY_SCRIPT: &str = r#"limit="$1"
for f in IDENTITY.md SOUL.md USER.md; do
  p="/sandbox/$f"
  if [ -e "$p" ] && [ ! -L "$p" ] && [ -f "$p" ]; then
    n=$(head -c "$limit" "$p" | wc -c | tr -d ' ')
    printf 'RIGHT_IDENTITY %s PRESENT %s\n' "$f" "$n"
    head -c "$limit" "$p"
    printf '\n'
  else
    printf 'RIGHT_IDENTITY %s ABSENT 0\n' "$f"
  fi
done"#;
```

- [ ] **Step 4: Rewrite `read_sandbox_identity_files` to one round trip**

Replace `read_sandbox_identity_files` (near `:90-150`) with a single-command read; on timeout / error / non-zero exit, label every file `sandbox_unreachable` and show host-mirror content; otherwise map each file via the parser + `identity_state`:

```rust
async fn read_sandbox_identity_files(
    state: &DashboardState,
    sandbox_exec: &SandboxExec,
) -> miette::Result<IdentityResponse> {
    let limit = (IDENTITY_PREVIEW_LIMIT_BYTES + 1).to_string();
    let command = [
        "sh",
        "-c",
        SANDBOX_READ_IDENTITY_SCRIPT,
        "dashboard-identity-read",
        limit.as_str(),
    ];
    let timeout = Duration::from_secs(super::DASHBOARD_SANDBOX_TIMEOUT_SECS);

    let exec_result = tokio::time::timeout(timeout, sandbox_exec.exec(&command)).await;
    let stdout = match exec_result {
        Ok(Ok((stdout, code))) if code == 0 => stdout,
        other => {
            let detail = match other {
                Ok(Ok((_, code))) => format!("sandbox identity read exited with code {code}"),
                Ok(Err(error)) => format!("sandbox identity read failed: {error:#}"),
                Err(_) => "sandbox identity read timed out".to_owned(),
            };
            let mut files = Vec::with_capacity(IDENTITY_FILE_NAMES.len());
            for name in IDENTITY_FILE_NAMES {
                files.push(host_fallback_file(&state.agent_dir, name, "sandbox_unreachable")?);
            }
            return Ok(IdentityResponse {
                agent: state.agent_name.clone(),
                source: "sandbox_unreachable".to_owned(),
                warning: Some(detail),
                files,
            });
        }
    };

    let parsed =
        identity_parse::parse_combined_identity_read(&stdout, IDENTITY_PREVIEW_LIMIT_BYTES);
    let mut files = Vec::with_capacity(IDENTITY_FILE_NAMES.len());
    let mut any_not_authored = false;
    for name in IDENTITY_FILE_NAMES {
        let entry = parsed.iter().find(|p| p.name == name);
        let sandbox_present = entry.map(|e| e.present).unwrap_or(false);
        let host_present = right_dashboard::fs_safety::is_regular_file_no_symlink(
            &state.agent_dir.join(name),
        )
        .unwrap_or(false);
        let file_state = identity_parse::identity_state(sandbox_present, host_present);
        if file_state == "not_authored" {
            any_not_authored = true;
        }
        let (exists, content_preview, truncated, path) = if sandbox_present {
            let e = entry.expect("present implies entry");
            (true, Some(e.content.clone()), e.truncated, format!("/sandbox/{name}"))
        } else if host_present {
            let summary = read_host_identity_file(
                &state.agent_dir,
                "host_mirror",
                "not_authored",
                name,
                IDENTITY_PREVIEW_LIMIT_BYTES,
            )
            .map_err(|e| miette::miette!("host mirror read failed for {name}: {e:#}"))?;
            (true, summary.content_preview, summary.truncated, name.to_owned())
        } else {
            (false, None, false, name.to_owned())
        };
        files.push(IdentityFileSummary {
            name: name.to_owned(),
            source: file_state.to_owned(),
            path,
            exists,
            content_preview,
            truncated,
        });
    }

    Ok(IdentityResponse {
        agent: state.agent_name.clone(),
        source: if files.iter().all(|f| f.source == "sandbox") {
            "sandbox".to_owned()
        } else {
            "mixed".to_owned()
        },
        warning: if any_not_authored {
            Some("Some identity files have not been authored in the sandbox yet.".to_owned())
        } else {
            None
        },
        files,
    })
}
```

Add the host-fallback helper near `host_mirror_or_unavailable` (near `:210`):

```rust
/// Host-mirror content tagged with an explicit per-file `state` label (used
/// when the sandbox is unreachable: show something, but never claim it is the
/// live copy).
fn host_fallback_file(
    agent_dir: &std::path::Path,
    name: &str,
    file_state: &str,
) -> Result<IdentityFileSummary, IdentityFilesError> {
    let mut summary = read_host_identity_file(
        agent_dir,
        file_state,
        file_state,
        name,
        IDENTITY_PREVIEW_LIMIT_BYTES,
    )?;
    summary.source = file_state.to_owned();
    Ok(summary)
}
```

> The single-file path `read_sandbox_identity_file` / `identity_file_response` (near `:49-88`, `:152-177`) handles clicking one file. Update its fallback vocabulary to match: success → `sandbox`; `Ok(None)` (exit 3) → `identity_parse::identity_state(false, host_present)`; `Err`/timeout → `sandbox_unreachable`. Keep it consistent with the parser-driven mapping above.

- [ ] **Step 5: Confirm `is_regular_file_no_symlink` is reachable**

Run: `rg -n "pub fn is_regular_file_no_symlink" crates/right-dashboard/src/fs_safety.rs`
Expected: it is `pub`. If not, make it `pub` (or add a thin `pub` wrapper) and adjust the call path used above to the real module path.

- [ ] **Step 6: Build + run identity tests**

Run: `devenv shell -- cargo test -p bot identity`
Expected: PASS (parser/mapping tests + any existing identity tests). Fix compile errors against the real in-sandbox runner signature and `IdentityFileSummary` fields.

- [ ] **Step 7: Commit**

```bash
git add crates/bot/src/telegram/dashboard/identity_parse.rs crates/bot/src/telegram/dashboard/identity.rs
git commit -m "fix(dashboard): coalesce identity sandbox read; distinguish unreachable from not-authored"
```

---

## Task 5: Identity frontend — labels, dedup, banner, retry

**Files:**
- Create: `crates/right-dashboard/frontend/src/components/identityLabels.ts`
- Create: `crates/right-dashboard/frontend/src/components/identityLabels.test.ts`
- Modify: `crates/right-dashboard/frontend/src/views/IdentityView.vue`
- Modify: `crates/right-dashboard/frontend/src/App.vue` (identity refresh wiring)

- [ ] **Step 1: Write the failing test for label/tone mapping**

Create `crates/right-dashboard/frontend/src/components/identityLabels.test.ts`:

```ts
import { describe, expect, it } from 'vitest'

import { identityLabel, identityTone } from './identityLabels'

describe('identityLabels', () => {
  it('maps each state to a human label', () => {
    expect(identityLabel('sandbox')).toBe('Live')
    expect(identityLabel('not_authored')).toBe('Not authored yet')
    expect(identityLabel('host_mirror')).toBe('Host mirror')
    expect(identityLabel('sandbox_unreachable')).toBe('Sandbox unreachable')
    expect(identityLabel('host')).toBe('Host')
    expect(identityLabel('missing')).toBe('Missing')
  })
  it('falls back to the raw value for unknown states', () => {
    expect(identityLabel('weird')).toBe('weird')
  })
  it('uses a warning tone only for unreachable/missing', () => {
    expect(identityTone('sandbox')).toBe('ok')
    expect(identityTone('host')).toBe('ok')
    expect(identityTone('host_mirror')).toBe('muted')
    expect(identityTone('not_authored')).toBe('muted')
    expect(identityTone('sandbox_unreachable')).toBe('bad')
    expect(identityTone('missing')).toBe('bad')
  })
})
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd crates/right-dashboard/frontend && pnpm test -- src/components/identityLabels.test.ts`
Expected: FAIL — module missing.

- [ ] **Step 3: Implement the mapping**

Create `crates/right-dashboard/frontend/src/components/identityLabels.ts`:

```ts
const LABELS: Record<string, string> = {
  sandbox: 'Live',
  not_authored: 'Not authored yet',
  host_mirror: 'Host mirror',
  sandbox_unreachable: 'Sandbox unreachable',
  host: 'Host',
  missing: 'Missing',
}

const TONES: Record<string, string> = {
  sandbox: 'ok',
  host: 'ok',
  host_mirror: 'muted',
  not_authored: 'muted',
  sandbox_unreachable: 'bad',
  missing: 'bad',
}

export function identityLabel(state: string): string {
  return LABELS[state] ?? state
}

export function identityTone(state: string): string {
  return TONES[state] ?? 'muted'
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd crates/right-dashboard/frontend && pnpm test -- src/components/identityLabels.test.ts`
Expected: PASS.

- [ ] **Step 5: Rewrite `IdentityView.vue` — dedup + labels + AsyncState + retry**

Replace `crates/right-dashboard/frontend/src/views/IdentityView.vue` with a single per-file row list (each row = selector + label pill; the separate `meta-grid` duplicate is removed); overall banner/retry derive from per-file state; detail panel uses `AsyncState`; add a `refresh` emit:

```vue
<script setup lang="ts">
import { computed } from 'vue'
import AsyncState from '../components/AsyncState.vue'
import { identityLabel, identityTone } from '../components/identityLabels'
import type { IdentityFileSummary, IdentityResponse } from '../types'

const props = defineProps<{
  identity: IdentityResponse | null
  selectedFile: IdentityFileSummary | null
  loading: boolean
  error: string | null
}>()

const emit = defineEmits<{
  selectFile: [name: string]
  refresh: []
}>()

const files = computed(() => props.identity?.files ?? [])
const unreachable = computed(() => files.value.some((f) => f.source === 'sandbox_unreachable'))
</script>

<template>
  <section class="two-column wide-main">
    <section class="panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">Identity</p>
          <h2>{{ identity?.agent ?? 'Agent' }}</h2>
        </div>
        <button v-if="unreachable" type="button" class="tool-button" @click="emit('refresh')">
          Retry
        </button>
      </header>
      <p v-if="identity?.warning" class="notice inline">{{ identity.warning }}</p>
      <div class="row-list">
        <button
          v-for="file in files"
          :key="file.name"
          type="button"
          class="data-row"
          :class="{ selected: selectedFile?.name === file.name }"
          @click="emit('selectFile', file.name)"
        >
          <span class="row-main"><strong>{{ file.name }}</strong></span>
          <span class="row-side">
            <span class="status-pill" :class="identityTone(file.source)">
              {{ identityLabel(file.source) }}
            </span>
          </span>
        </button>
        <p v-if="files.length === 0 && !loading" class="muted-line">No identity files</p>
      </div>
    </section>

    <aside class="panel detail-panel">
      <header class="panel-head">
        <div>
          <p class="eyebrow">File</p>
          <h2>{{ selectedFile?.name ?? 'None selected' }}</h2>
        </div>
        <span
          v-if="selectedFile"
          class="status-pill"
          :class="identityTone(selectedFile.source)"
        >{{ identityLabel(selectedFile.source) }}</span>
      </header>
      <AsyncState :loading="loading" :error="error" :empty="!selectedFile" empty-text="No file selected">
        <template v-if="selectedFile">
          <p class="muted-line">{{ selectedFile.path }}</p>
          <pre v-if="selectedFile.exists">{{ selectedFile.content_preview }}<template v-if="selectedFile.truncated">
... truncated
</template></pre>
          <p v-else class="muted-line">{{ identityLabel(selectedFile.source) }}</p>
        </template>
      </AsyncState>
    </aside>
  </section>
</template>
```

> `.status-pill` tone classes (`ok`/`bad`/`muted`/`active`) are global styles already used by `StatusPill.vue`; reusing the class names keeps styling consistent. The old `StatusPill` import is dropped because the pills now use `identityTone` directly — confirm nothing else in this file references `StatusPill`.

- [ ] **Step 6: Wire the `refresh` emit in `App.vue`**

In `App.vue`, the `<IdentityView ... />` block (near `:402-409`) gains `@refresh="refreshIdentity"`:

```vue
    <IdentityView
      v-else-if="activeTab === 'identity'"
      :identity="identityData"
      :selected-file="selectedIdentityFile"
      :loading="loadingIdentity"
      :error="identityError"
      @select-file="selectIdentityFile"
      @refresh="refreshIdentity"
    />
```

> `refreshIdentity` already exists in `App.vue` (near `:224`). Confirm it resets `identityError` on entry (siblings do); if not, add that reset so Retry can clear a prior error.

- [ ] **Step 7: Run tests + typecheck**

Run: `cd crates/right-dashboard/frontend && pnpm test && pnpm typecheck`
Expected: PASS, no type errors.

- [ ] **Step 8: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/identityLabels.ts \
  crates/right-dashboard/frontend/src/components/identityLabels.test.ts \
  crates/right-dashboard/frontend/src/views/IdentityView.vue \
  crates/right-dashboard/frontend/src/App.vue
git commit -m "feat(dashboard): humanize identity states, dedup list, add retry"
```

---

## Task 6: Skills — collapsible groups with counts

**Files:**
- Modify: `crates/right-dashboard/frontend/src/views/SkillsView.vue`

- [ ] **Step 1: Wrap each group in `CollapsibleSection`; AsyncState the detail**

Add imports (near `:2-6`):

```ts
import AsyncState from '../components/AsyncState.vue'
import CollapsibleSection from '../components/CollapsibleSection.vue'
```

Replace the per-group `<article class="panel">` loop (near `:88-116`) with:

```vue
      <CollapsibleSection
        v-for="group in skillGroups"
        :key="group"
        :title="group"
        :count="skillsFor(skills, group).length"
      >
        <div class="row-list">
          <button
            v-for="skill in skillsFor(skills, group)"
            :key="skill.name"
            type="button"
            class="data-row"
            :class="{ selected: selectedSkillName === skill.name }"
            @click="emit('selectSkill', skill)"
          >
            <span class="row-main">
              <strong>{{ skill.name }}</strong>
              <small>{{ skill.description ?? skill.path }}</small>
            </span>
            <span v-if="hasLifecycleRow(skill)" class="row-side">
              <span>{{ pinLabel(skill) }}</span>
              <small>{{ lifecycleLabel(skill.state) }}</small>
            </span>
          </button>
          <p v-if="skillsFor(skills, group).length === 0" class="muted-line">None</p>
        </div>
      </CollapsibleSection>
```

Replace the detail loading/empty/error trio (near `:130-132`) with `AsyncState` wrapping the existing `<template v-if="selectedSkill">` content (keep that inner content — path, pin toolbar, meta-grid, `<pre>` — verbatim):

```vue
      <AsyncState
        :loading="loading"
        :error="error ?? selectedPinError"
        :empty="!selectedSkill"
        empty-text="No skill selected"
      >
        <template v-if="selectedSkill">
          <!-- existing path / pin toolbar / meta-grid / pre block unchanged -->
        </template>
      </AsyncState>
```

> The `skills?.warning` notice (near `:87`) stays above the sections. Drop the per-group `StatusPill` (each group now shows a count badge). If `StatusPill` becomes unused, remove its import.

- [ ] **Step 2: Run tests + typecheck**

Run: `cd crates/right-dashboard/frontend && pnpm test && pnpm typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/right-dashboard/frontend/src/views/SkillsView.vue
git commit -m "feat(dashboard): collapse skill groups by default with counts"
```

---

## Task 7: Overview — reorder + inline accordion signal detail

**Files:**
- Modify: `crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue`
- Modify: `crates/right-dashboard/frontend/src/views/OverviewView.vue`

- [ ] **Step 1: Make `SignalTimeline` rows expandable with inline detail**

Rewrite `SignalTimeline.vue` so a tapped row shows an inline detail block beneath it (single-open). The detail renders *after* the button, so the tapped row itself never moves (anchored). It still emits `select`; the open row is whichever matches `selectedId`.

```vue
<script setup lang="ts">
import StatusPill from '../StatusPill.vue'
import { money, shortDate } from '../../format'
import type { DashboardSignal } from '../../types'

defineProps<{
  signals: DashboardSignal[]
  selectedId: string | null
}>()

const emit = defineEmits<{
  select: [signal: DashboardSignal]
}>()
</script>

<template>
  <section class="panel chart-panel">
    <header class="panel-head">
      <div>
        <p class="eyebrow">Signals</p>
        <h2>Recent changes</h2>
      </div>
    </header>

    <div v-if="signals.length === 0" class="chart-empty">No recent signals</div>
    <template v-for="signal in signals" v-else :key="signal.id">
      <button
        type="button"
        class="data-row tall"
        :class="{ selected: selectedId === signal.id }"
        :aria-expanded="selectedId === signal.id"
        @click="emit('select', signal)"
      >
        <span class="row-main">
          <strong>{{ signal.title }}</strong>
          <span>{{ signal.detail ?? signal.source ?? signal.kind }}</span>
          <small>{{ shortDate(signal.occurred_at) }}</small>
        </span>
        <span class="row-side">
          <StatusPill :status="signal.severity" />
          <small v-if="signal.cost_usd !== null">{{ money(signal.cost_usd) }}</small>
        </span>
      </button>
      <dl v-if="selectedId === signal.id" class="meta-grid compact signal-detail">
        <div><dt>When</dt><dd>{{ shortDate(signal.occurred_at) }}</dd></div>
        <div><dt>Source</dt><dd>{{ signal.source ?? 'none' }}</dd></div>
        <div><dt>Skill</dt><dd>{{ signal.related_skill_name ?? 'none' }}</dd></div>
        <div><dt>Cost</dt><dd>{{ money(signal.cost_usd) }}</dd></div>
        <div><dt>Kind</dt><dd>{{ signal.kind }}</dd></div>
        <div v-if="signal.related_run_id"><dt>Run</dt><dd>{{ signal.related_run_id }}</dd></div>
        <div v-if="signal.related_report_id"><dt>Report</dt><dd>{{ signal.related_report_id }}</dd></div>
        <div v-if="signal.detail" class="signal-detail-text"><dt>Detail</dt><dd>{{ signal.detail }}</dd></div>
      </dl>
    </template>
  </section>
</template>

<style scoped>
.signal-detail {
  padding: 8px 12px 14px;
  background: var(--tg-theme-secondary-bg-color, rgba(127, 127, 127, 0.1));
  border-radius: 0 0 10px 10px;
  margin-bottom: 8px;
}
.signal-detail-text {
  grid-column: 1 / -1;
}
</style>
```

- [ ] **Step 2: Reorder `OverviewView.vue` and drop the side detail panel**

Edits to `OverviewView.vue`:
1. Move `<CostLearningRiver ... />` to the **top** of `<template>`, before `metric-grid`.
2. Remove the right-hand `<aside class="panel detail-panel">` (near `:86-131`) and the `two-column` wrapper (near `:79`,`:132`); render `<SignalTimeline>` directly (detail is now inline).
3. Simplify selection state to a single `selectedId` ref; `selectSignal` toggles; `selectMarker` maps a river marker to its matching signal.

Resulting `<template>`:

```vue
<template>
  <CostLearningRiver :river="overview?.cost_learning_river ?? null" @select-marker="selectMarker" />

  <section class="metric-grid">
    <!-- the six MetricCard entries unchanged (near :64-69) -->
  </section>

  <section v-if="overview?.warnings.length" class="notice">
    <strong>Partial data</strong>
    <span v-for="warning in overview.warnings" :key="`${warning.source}:${warning.kind}:${warning.message}`">
      {{ warning.message }}
    </span>
  </section>

  <SignalTimeline
    :signals="overview?.signals ?? []"
    :selected-id="selectedId"
    @select="selectSignal"
  />
</template>
```

`<script setup>` selection logic:

```ts
const selectedId = ref<string | null>(null)

function selectSignal(signal: DashboardSignal): void {
  selectedId.value = selectedId.value === signal.id ? null : signal.id
}

function selectMarker(marker: LearningMarker): void {
  const match = (props.overview?.signals ?? []).find((s) => s.id === marker.id)
  selectedId.value = match ? match.id : null
}
```

> Remove now-unused symbols only if your edits made them unused (`selectedKind`, `selectedSignal`, `selectedMarker`, `selectedEyebrow`, the `watch`, and any `StatusPill`/`money`/`shortDate` imports no longer referenced after the panel removal). Keep `MetricCard`, `CostLearningRiver`, `SignalTimeline`. `props` must remain accessible to `selectMarker` (keep the `const props = defineProps(...)` binding).

- [ ] **Step 3: Run tests + typecheck**

Run: `cd crates/right-dashboard/frontend && pnpm test && pnpm typecheck`
Expected: PASS, no unused-import/type errors. Fix any flagged unused symbols.

- [ ] **Step 4: Commit**

```bash
git add crates/right-dashboard/frontend/src/components/charts/SignalTimeline.vue \
  crates/right-dashboard/frontend/src/views/OverviewView.vue
git commit -m "feat(dashboard): cost/learning on top; inline accordion signal detail"
```

---

## Task 8: Cross-cutting loading states (Usage, Health, Activity, Overview)

**Files:**
- Modify: `crates/right-dashboard/frontend/src/App.vue`
- Modify: `crates/right-dashboard/frontend/src/views/UsageView.vue`
- Modify: `crates/right-dashboard/frontend/src/views/HealthView.vue`
- Modify: `crates/right-dashboard/frontend/src/views/ActivityView.vue`
- Modify: `crates/right-dashboard/frontend/src/views/OverviewView.vue`

- [ ] **Step 1: Derive loading/error for Overview & Usage in `App.vue`**

Add computeds in `App.vue` `<script setup>` (near the other `computed`s):

```ts
const overviewLoading = computed(() => dashboardData.value === null && connectionState.value === 'loading')
const overviewError = computed(() =>
  dashboardData.value === null && (connectionState.value === 'offline' || connectionState.value === 'locked')
    ? 'Dashboard unavailable'
    : null,
)
const usageLoading = computed(() => usageData.value === null && connectionState.value === 'loading')
const usageError = computed(() =>
  usageData.value === null && (connectionState.value === 'offline' || connectionState.value === 'locked')
    ? 'Usage unavailable'
    : null,
)
```

Pass them through:

```vue
    <OverviewView
      v-if="activeTab === 'overview'"
      :overview="dashboardData"
      :activity="activityData"
      :loading="overviewLoading"
      :error="overviewError"
    />
    <UsageView
      v-else-if="activeTab === 'usage'"
      :usage="usageData"
      :loading="usageLoading"
      :error="usageError"
    />
```

- [ ] **Step 2: Accept loading/error in OverviewView & wrap content**

In `OverviewView.vue` extend `defineProps` (keep the `const props =` binding from Task 7):

```ts
const props = defineProps<{
  overview: DashboardOverviewResponse | null
  activity: OverviewResponse | null
  loading: boolean
  error: string | null
}>()
```

Wrap the whole template body in `AsyncState` (import it). The metric grid / river / timeline from Task 7 become the slot:

```vue
<template>
  <AsyncState :loading="loading" :error="error" :empty="overview === null && !loading" empty-text="No overview data">
    <!-- CostLearningRiver / metric-grid / warnings / SignalTimeline (Task 7) -->
  </AsyncState>
</template>
```

- [ ] **Step 3: Accept loading/error in UsageView & wrap**

Read `UsageView.vue` first to see its current prop list and root element. Add `loading: boolean` and `error: string | null` to `defineProps`, import `AsyncState`, and wrap the existing root section(s):

```vue
<template>
  <AsyncState :loading="loading" :error="error" :empty="usage === null && !loading" empty-text="No usage data">
    <!-- existing UsageView body (windows / charts / breakdown) -->
  </AsyncState>
</template>
```

> The per-chart `chart-empty` "No usage data" (e.g. `UsageSpendChart.vue:107`) stays — it now only appears once `usage` is loaded but a series is empty, not during the initial fetch.

- [ ] **Step 4: Swap Health placeholders to spinner-aware text**

In `HealthView.vue`, import `Spinner` and replace the `'not loaded'` ternaries (near `:31`,`:71`) so a spinner shows during the initial load:

```vue
<h2 v-if="loadingDoctor && !doctor"><Spinner /></h2>
<h2 v-else>{{ doctor ? `${doctor.pass_count}/${doctor.pass_count + doctor.warn_count + doctor.fail_count}` : 'not loaded' }}</h2>
```

Apply the analogous change to the sandbox panel (near `:71`) using `loadingSandbox` and `sandbox`.

- [ ] **Step 5: Activity detail loading → spinner**

In `ActivityView.vue`, replace the two `<p v-if="loadingDetail" class="muted-line">Loading</p>` lines (near `:88`,`:160`) with `<div v-if="loadingDetail" class="async-loading"><Spinner /></div>` (import `Spinner`). Leave `Log unavailable` messages (they describe loaded data, not loading).

> If wrapping the whole detail block in `AsyncState` is clean in this file, prefer that (`empty = !selectedRun`, `error = detailError`); otherwise the minimal spinner swap above is acceptable.

- [ ] **Step 6: Run tests + typecheck + build**

Run: `cd crates/right-dashboard/frontend && pnpm test && pnpm typecheck && pnpm build`
Expected: PASS; production build succeeds.

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/frontend/src/App.vue \
  crates/right-dashboard/frontend/src/views/OverviewView.vue \
  crates/right-dashboard/frontend/src/views/UsageView.vue \
  crates/right-dashboard/frontend/src/views/HealthView.vue \
  crates/right-dashboard/frontend/src/views/ActivityView.vue
git commit -m "feat(dashboard): route loading/empty/error through AsyncState (no more 'not loaded' flash)"
```

---

## Task 9: ARCHITECTURE.md — Dashboard frontend primitives rule

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Add the prescriptive subsection**

Insert near the "Brand-conformant CLI output" / "Telegram message UX" UI-rule cluster:

```markdown
## Dashboard frontend primitives

`right-dashboard` Vue views MUST render loading/empty/error through
`components/AsyncState.vue` (backed by the pure `components/asyncState.ts`
resolver) and MUST render collapsible grouped lists through
`components/CollapsibleSection.vue`. Raw placeholder text (`'not loaded'`,
`'unavailable'`, ad-hoc `v-if="loading"` Loading lines) in a view is a
review-blocking defect — it reintroduces the loading-flash these primitives
exist to prevent. Identity per-file state labels go through
`components/identityLabels.ts`, never raw enum codes.
```

- [ ] **Step 2: Verify the character budget**

Run: `wc -c ARCHITECTURE.md`
Expected: under 40000. If over, move a descriptive paragraph to a `docs/architecture/*.md` satellite in this same commit (per AGENTS.md "Architecture docs split").

- [ ] **Step 3: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs(architecture): require AsyncState/CollapsibleSection in dashboard views"
```

---

## Task 10: Final verification

**Files:** none (verification only)

- [ ] **Step 1: Full Rust workspace test (mandatory)**

Run: `devenv shell -- cargo test --workspace`
Expected: PASS. Record any pre-existing unrelated failures; the migration and identity tests must pass.

- [ ] **Step 2: Full frontend gate**

Run: `cd crates/right-dashboard/frontend && pnpm test && pnpm typecheck && pnpm build`
Expected: all PASS; build emits to `../static/dashboard`.

- [ ] **Step 3: Workspace build (debug)**

Run: `devenv shell -- cargo build --workspace`
Expected: PASS.

- [ ] **Step 4: Final review pass**

Run the `rust-dev:review-rust-code` subagent over the bot + right-db changes; convert findings to TODOs and fix one by one. Confirm no bare `std::fs::write` was added in codegen, and no error-swallowing (`unwrap_or_default()` / `.ok()` / `let _ =`) in the new identity backend paths.

---

## Self-Review notes (planner)

- **Spec coverage:** §A primitives → Tasks 2,3; §B Overview → Task 7; §C Usage delete → Task 1; §D Identity backend → Task 4; §D Identity frontend → Task 5; §E Skills → Task 6; §F loading states → Task 8; §G ARCHITECTURE → Task 9. All covered.
- **Type consistency:** identity state strings (`sandbox`/`sandbox_unreachable`/`host_mirror`/`not_authored`/`host`/`missing`) are produced in `identity_parse::identity_state` + `read_sandbox_identity_files` (Rust) and consumed by `identityLabels.ts` (TS) — same six tokens both sides. `resolveAsyncState`/`AsyncState` prop names (`loading`/`error`/`empty`/`emptyText`) match across helper, component, and all call sites.
- **Confirm-against-real-code notes** (column lists, helper visibility, submodule declaration site, exact line numbers) are flagged inline with `>` and must be checked before running the affected step — line numbers are from the spec-time snapshot and may drift.
