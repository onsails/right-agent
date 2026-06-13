# cargo-nextest integration — design

**Status:** design, 2026-06-13
**Author:** brainstorming session (Andrey + Claude)

## Goal

Adopt `cargo-nextest` as the recommended test runner to cut CI wall-clock and
the local dev loop, **without losing** the concurrency guarantees the suite
relies on — and rewrite the few guarantees that nextest's execution model
forces us to change onto cross-process / nextest-native primitives. Also
harden the live-sandbox (`ignored`) job with nextest's per-test process
isolation and clean per-test timeouts.

## Why nextest changes anything

`cargo test` compiles one test binary per crate and runs all its tests **as
threads in a single process**. `cargo nextest run` runs **each test in its own
process**. Consequences for *this* repo:

- **File-lock guarantees survive unchanged.** `acquire_sandbox_slot` and
  `acquire_test_name_lock` (`crates/right-openshell/src/openshell.rs`) are
  `flock`-style advisory locks keyed on `$TMPDIR` paths — cross-process by
  construction, identical behavior under both runners.
- **In-process `static` serialization either self-heals or breaks.** A `static`
  mutex/semaphore that exists *only* to serialize threads within one test
  binary stops serializing under nextest (separate processes don't share a
  `static`). Whether that matters depends on what it protected:
  - If it guarded a **process-global `static`** (a semaphore, a registry), each
    nextest process gets its own fresh copy → the contention vanishes → the
    guard becomes unnecessary. **Self-heals.**
  - If it coordinated an **expensive shared external resource** (a single live
    sandbox reused across tests), nextest's process isolation **breaks** the
    sharing and can deadlock on a lifetime lock. **Must be rewritten.**
- **No doctests in the repo (0 today)**, so nextest's lack of doctest support
  costs nothing. A near-free `cargo test --doc` guard step future-proofs this.

## Audit of current serialization

| Mechanism | Kind | Under nextest | Action |
|---|---|---|---|
| `acquire_sandbox_slot` | cross-process file lock (host-global, cross-worktree) | works | **keep** as host-global backstop |
| `acquire_test_name_lock(name)` | cross-process file lock | works | **keep** |
| `ARCHIVE_TEST_MUTEX` (`crates/bot/src/telegram/archive.rs`) | in-process `tokio::Mutex` guarding the process-global `ARCHIVE_WRITE_PERMITS` semaphore + a 250 ms timing assertion | self-heals (own process → own semaphore → no contention) | **keep mutex** for `cargo test` correctness; **no** nextest group needed |
| `shared_test_sandbox` `OnceCell` (`crates/right-openshell/src/openshell_tests.rs:638`) | in-process shared **live sandbox** under fixed name `right-test-shared`, `&'static` never drops | **breaks**: each process re-boots; lifetime name lock serializes processes → full delete-create loop | **rewrite** as cross-process create-once fixture (below) |
| `HOOK_INSTALLED` `OnceLock` (`test_cleanup.rs`) | once-per-process cleanup-hook install | self-heals (each process installs its own) | no change |
| `PROCESS_ENV_LOCK` / `PathGuard` (`test_support.rs`) | in-process env mutation lock | self-heals (own process → own env) | no change |

The only guarantee nextest **forces** us to rewrite is the shared sandbox.

## Decisions (from brainstorming)

1. **Optimize CI wall-clock + local dev loop, and harden the sandbox job** with
   isolation/timeouts (it's latency-bound, so it won't get much faster).
2. **Keep the file locks** as the host-global, cross-worktree backstop; layer
   nextest test-groups on top for the precise per-prefix caps. Belt-and-
   suspenders.
3. **Both runners stay correct.** `cargo nextest run` is the documented fast
   path; `cargo test` remains valid (it cannot be removed — it's built into
   Cargo) and never silently wrong. Project docs recommend nextest only.
4. **`retries = 1` on the `ci` (workspace) profile** to absorb the two known
   parallel-load flakes (cc/invocation pid race, dashboard warn-count);
   **`retries = 0` on `ci-ignored`** so a flaky sandbox test surfaces instead of
   triggering an expensive silent re-boot.
5. **Preserve the single-boot shared sandbox** — per-test sandboxes (a
   delete-create loop) are unacceptably slow. Rewrite it cross-process.

## Design

### A. Tooling & config

- Add `cargo-nextest` to `devenv.nix` packages.
- New `.config/nextest.toml`:
  - **Profiles:** `default` (local), `ci` (workspace job), `ci-ignored`
    (sandbox job).
  - **`fail-fast = false`** on `ci` and `ci-ignored` (mirrors today's
    `--no-fail-fast`).
  - **`retries`:** `ci = 1`, `ci-ignored = 0`, `default = 0`.
  - **`[test-groups]`:** `sandbox-claude { max-threads = 1 }`,
    `sandbox-openshell { max-threads = 2 }`.
  - **`[[profile.ci-ignored.overrides]]`** mapping filter expressions to groups
    and per-group `slow-timeout`:
    - `filter = 'test(/ci_claude_/)'` → `test-group = "sandbox-claude"`
    - `filter = 'test(/ci_openshell_/)'` → `test-group = "sandbox-openshell"`
    - per-group `slow-timeout = { period = "120s", terminate-after = 6 }` → a
      ~12 min hard kill, comfortably above the worst-case in-test setup (READY
      360 s + SSH 120 s = 480 s, `RIGHT_TEST_SANDBOX_{READY,SSH}_TIMEOUT_SECS`)
      so nextest is the backstop and never preempts the test's own diagnostic.
      Tunable; the invariant is kill-time > setup + expected work.
- `#[ignore]`, the `ci_{openshell,claude,stt}_` prefixes, and
  `crates/right/tests/ci_ignored_contract.rs` are **untouched**. nextest
  respects `#[ignore]` by default; the prefixes now double as filter-expression
  selectors. The contract test remains the gate.

### B. Forced code change — cross-process shared sandbox

Replace the `OnceCell`-based `shared_test_sandbox` with a **non-owning,
cross-process create-once** fixture in `crates/right-openshell/src/test_support.rs`.

New type `SharedSandboxRef { name, mtls_dir }` — same usable surface as the
relevant `TestSandbox` methods (`name`, `exec`, `exec_with_timeout`, plus
whatever the upload/download/verify suite calls through `openshell::` free
functions) **minus** delete-on-`Drop` and **minus** the lifetime name lock.

```text
async fn shared_sandbox(label) -> SharedSandboxRef:
    runid = env("RIGHT_TEST_RUN_ID").unwrap_or_else(getppid)   // zero-config per-run id
    name  = format!("right-test-shared-{label}-{runid}")
    _create_lock = acquire_test_name_lock(format!("shared-create-{name}"))  // held only here
    client = connect_grpc(mtls_dir)
    if sandbox_exists(name) && is_sandbox_ready(name):
        wait_for_ssh(name, short)         // cheap if already up
        return SharedSandboxRef { name, mtls_dir }   // attach — no boot
    if sandbox_exists(name):
        delete_sandbox(name); wait_for_deleted(name) // half-booted / unhealthy leftover
    slot = acquire_sandbox_slot()         // held ONLY during boot
    spawn_sandbox(name); wait_for_ready(name); wait_for_ssh(name)
    drop(slot)
    return SharedSandboxRef { name, mtls_dir }
    // _create_lock drops on return
```

Repoint the upload/download/verify tests from `shared_test_sandbox` to
`shared_sandbox("<label>")`. Distinct sandbox-side paths per test are **already
required** today; that invariant carries over unchanged (now 2-wide concurrent
access to the one shared sandbox, which gRPC handles fine).

#### Stale-lock / stale-sandbox safety (load-bearing)

- **Advisory locks auto-release on process death.** The coordination lock is a
  `std::fs::File` `flock` tied to the open FD; panic/SIGKILL/abort closes the FD
  and frees the lock. The lock *file* persists; the lock *state* never outlives
  its process. Same property the existing slot/name locks already rely on.
- **Lock held only across create-or-attach**, sub-second on the attach path —
  no lifetime holds to go stale.
- **`slow-timeout terminate-after`** kills a *live* process wedged inside the
  create block (e.g. a hung `wait_for_ready` past its own timeout) → FD closes →
  lock frees. Backstop and lock auto-release compose.
- **Liveness gate, not existence gate:** attach only on `exists && is_ready`
  (+ ssh). Anything sick is deleted→recreated; never attach to a half-booted
  sandbox.
- **Run-scoped name eliminates cross-run/cross-worktree thrash.** Name and lock
  key both embed `runid`, so two concurrent runs/worktrees never contend on the
  lock and **never delete each other's live sandbox** (the "B mistakes A's live
  sandbox for a stale leftover" failure cannot occur). Within one run, all
  processes share `getppid` → one name → one boot, the rest attach. (PID reuse
  across distant invocations is harmless: an old id's sandbox is a non-matching
  leftover, gated out by run-scoped naming + the liveness check.)
- **Slot held only during boot:** idle shared sandbox doesn't shrink the
  concurrency cap. Max live sandboxes = slot cap + 1 (conscious, documented).
- **Leftovers:** CI runners are ephemeral → no accumulation. Locally, dead-run
  `right-test-shared-*-<oldid>` sandboxes are removed by a documented prune; the
  fixture must **not** GC other run-ids mid-run (a different id may be live).
- **Single cross-process code path, no `OnceCell`.** Under `cargo test` (one
  process) the first call boots and later calls hit the fast gRPC liveness
  attach (~ms), so the lost in-process cache is immaterial.

### C. Serialization mapping (net effect)

- Host-global concurrency: `acquire_sandbox_slot` file lock, env
  `RIGHT_MAX_CONCURRENT_SANDBOX_TESTS = 2` in the ignored job (unchanged).
- Precise per-prefix concurrency within a run: nextest test-groups
  (`sandbox-claude = 1`, `sandbox-openshell = 2`) — expresses what the single
  env knob could not.
- Fixed-name resources: `acquire_test_name_lock` (unchanged).
- `ARCHIVE_TEST_MUTEX` kept for `cargo test`; nextest self-heals it.

### D. CI restructure (`.github/workflows/tests.yml`)

**workspace job:**
- `cargo nextest run --profile ci --workspace --locked` (replaces
  `cargo test --workspace --lib --bins --tests --no-fail-fast`).
- Keep the dashboard-bundle-freshness check (nextest filter form) and clippy.
- Add `cargo test --doc --workspace --locked` as a near-instant future-doctest
  guard.

**ignored job:**
- **STT stays its own step** (it runs *before* OpenShell install):
  `cargo nextest run --profile ci-ignored --run-ignored only -E 'test(/ci_stt_/)'`.
- **Merge claude + openshell into one run** (was two steps with hand-tuned
  `RIGHT_MAX_CONCURRENT_SANDBOX_TESTS` env):
  `cargo nextest run --profile ci-ignored --features right-openshell/test-support
  --run-ignored only -E 'test(/ci_claude_/) | test(/ci_openshell_/)'`,
  `RIGHT_MAX_CONCURRENT_SANDBOX_TESTS=2`, `RIGHT_TEST_RUN_ID=$GITHUB_RUN_ID`.
  Per-prefix concurrency comes from the test-groups; `fail-fast = false` reports
  both groups in one pass.
- The `continue-on-error` + aggregate "Check ignored test results" step
  collapses to: STT step result + the merged-run result. nextest's exit status
  already aggregates within a run.

### E. Docs

- `AGENTS.rust.md` / `AGENTS.md` / project `CLAUDE.md` verification cadence:
  recommend `devenv shell -- cargo nextest run -p <crate> <filter>` for the dev
  loop and `devenv shell -- cargo nextest run --workspace` for final
  verification. Note `cargo test` stays valid but is not the documented fast
  path. Keep `cargo test --doc` mentioned for doctests.
- `devenv.nix` `enterTest`: `cargo nextest run --workspace` + clippy
  (+ `cargo test --doc --workspace` if/when doctests exist).
- The "Worktree binary for `right`" and sandbox-debug conventions are
  unaffected.

## Risks & non-goals

- **Risk:** nextest filter-expression syntax drift across versions. Mitigation:
  pin a known-good `cargo-nextest` via devenv; the `-E 'test(/.../)'` form is
  stable.
- **Risk:** `slow-timeout terminate-after` set too low preempts a legitimately
  slow cold-runner boot. Mitigation: derive it from
  `RIGHT_TEST_SANDBOX_READY_TIMEOUT_SECS` with headroom.
- **Risk:** local leftover `right-test-shared-*` sandboxes. Mitigation:
  documented prune; clearly namespaced; harmless.
- **Non-goal:** removing `#[ignore]` in favor of pure nextest filters. `#[ignore]`
  keeps sandbox tests out of the default local/CI run — orthogonal to the runner
  and still correct.
- **Non-goal:** nextest setup-scripts for the shared sandbox. The cross-process
  fixture works under both runners with one code path; setup-scripts are
  nextest-only and would force a dual path. Revisit only if a future fixture
  genuinely needs nextest-only setup.
- **Non-goal:** changing `ARCHIVE_WRITE_PERMITS` production behavior.

## Verification cadence

Per project rules — targeted intermediate checks, one final full workspace run:

1. Baseline before changes: `devenv shell -- cargo nextest run -p right-openshell`
   (after adding nextest) to confirm the runner builds/runs the existing suite.
2. After the shared-sandbox rewrite: targeted live run of the upload/download/
   verify suite via nextest with `--run-ignored only -E 'test(/ci_openshell_/)'`
   at `max-threads = 2`, asserting **one** boot (log inspection) and green tests.
   Re-run twice back-to-back to prove run-scoped naming reuses within a run and
   recreates across runs without thrash.
3. After CI restructure: push to a branch, confirm both jobs green and the
   merged ignored step reports both groups.
4. **Final (mandatory):** `devenv shell -- cargo nextest run --workspace` plus
   `devenv shell -- cargo test --doc --workspace` and
   `devenv shell -- cargo clippy --workspace -- -D warnings` from the worktree.

## Rollout

1. devenv + `.config/nextest.toml` (profiles, groups, timeouts, retries).
2. Shared-sandbox rewrite (`SharedSandboxRef` + repoint consumers).
3. CI workflow restructure.
4. Docs (`AGENTS*.md`, `CLAUDE.md`, `devenv.nix enterTest`).

Each step is independently landable; (2) is the only behavioral code change and
carries its own targeted live verification.
