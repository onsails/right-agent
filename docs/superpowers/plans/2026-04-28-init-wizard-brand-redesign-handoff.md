# Init Wizard Brand Redesign — Handoff

**Read this first when resuming after compaction.**

## Locations

| What | Path |
|---|---|
| **Plan** | `docs/superpowers/plans/2026-04-28-init-wizard-brand-redesign.md` (31 tasks across 6 steps) |
| **Spec** | `docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md` |
| **Worktree** | `.worktrees/init-wizard-brand-redesign/` |
| **Branch** | `feature/init-wizard-brand-redesign` (off `master`) |
| **This handoff** | `docs/superpowers/plans/2026-04-28-init-wizard-brand-redesign-handoff.md` |

`cd` into the worktree before resuming. All commands below assume that working directory.

## Execution mode

Subagent-driven (`superpowers:subagent-driven-development`). One implementer subagent per task, two-stage review per task (spec compliance + code quality). Plan was created via `superpowers:writing-plans`.

## Progress

| Step | Tasks | Status |
|---|---|---|
| 1 | 1–9 | ✅ `right-agent::ui` module shipped (atoms, theme, line/block, splash, header, recap, writer, error sentinel) |
| 2 | 10–12 | ✅ `cmd_doctor` migrated to ui::Block; integration tests for doctor |
| 3 | 13–15 | ✅ `cmd_status` migrated; `BlockAlreadyRendered` wired into `main::handle_dispatch` |
| 4 | 16–21 | ⏳ **next**: `right init` redesign — splash + dependency probe + section headers + recap + voice rewrites |
| 5 | 22–27 | ⏳ agent-init + config + sandbox/network/STT/memory copy + `PROMPT_LABELS` regression |
| 6 | 28–31 | ⏳ `--no-color` flag + brand-conformance lint + final clippy/build |

**HEAD on resume:** `0d627dff` (after Task 14, status migration).

## Key commit chain

```
0d627dff feat(status): brand-conformant rail+glyph block          ← HEAD
583fda7c test(doctor): rename + ascii-fallback assertions
10d93cdb feat(doctor): render diagnostics as brand-conformant block
9836345f feat(ui): writers + BlockAlreadyRendered sentinel docs
5e266291 feat(ui): recap builder with column-aligned status block
b32f8332 docs(ui): add doc comments on splash and section pub fns
60247c11 feat(ui): splash + section header
1a535d2c docs(ui): doc comment on Line struct
201e20a4 feat(ui): status line + block builder with column alignment
e5b20021 feat(ui): rail + semantic glyphs with three theme tiers
f012ec38 docs(spec): Rail::* return String, not &'static str
6c1682fb refactor(ui): tighten theme detection visibility to pub(crate)
30cb1b0b feat(ui): theme detection (color/mono/ascii)
7ca12085 feat(ui): scaffold right-agent::ui module skeleton
```

## Verification gates currently passing

```
cargo test -p right-agent ui:: --no-fail-fast        # 46 unit tests pass
cargo test -p right --test wizard_brand              # 4 integration tests pass
cargo build --workspace                              # clean
cargo clippy --workspace --all-targets -- -D warnings # zero warnings
```

## Deviations from the plan (locked in)

1. **`IsTty` instead of `IsTerminal`.** `std::io::IsTerminal` is sealed in the active toolchain; tests can't impl it. We have `pub(crate) trait IsTty { fn is_tty(&self) -> bool; }` with `RealTty` delegating to `stdout().is_terminal()`. Same testability outcome.
2. **`Rail::prefix/mark/blank` return `String`, not `&'static str`.** The Color path produces ANSI-wrapped owned strings. Spec doc patched at `f012ec38`.
3. **Mono integration tests omitted.** `assert_cmd` runs the binary non-TTY → theme detection forces `Ascii` regardless of `NO_COLOR`. The `Mono` branch is fully covered by unit tests in `theme_tests.rs`, `splash_tests.rs`, `atoms_tests.rs`, `recap_tests.rs`. The original plan's "mono no_ansi" integration tests should be skipped or rewritten as Ascii assertions when Tasks 29+ land.
4. **Test injection traits/structs are `pub(crate)`.** `EnvGet`, `IsTty`, `RealEnv`, `RealTty`, `detect_with` — internal-only. `Theme`, `detect`, `Rail`, `Glyph`, `Line`, `Block`, `status`, `splash`, `section`, `Recap`, `BlockAlreadyRendered`, `stdout`, `stderr` are `pub` and re-exported from `crates/right-agent/src/ui/mod.rs`.
5. **`BlockAlreadyRendered` derives `miette::Diagnostic`** so `.into()` converts to `miette::Report`. Not in the original spec/plan, added in Task 14.
6. **Bot crate also called the removed `Display for DoctorCheck`.** Updated in `crates/bot/src/telegram/handler.rs`'s `handle_doctor`. Out-of-plan scope expansion, but unavoidable to keep the workspace building. Noted in Task 11's commit.

## Critical pieces that must be reused

- **`handle_dispatch` helper in `crates/right/src/main.rs`** (top of file). Wraps the CLI dispatch result; intercepts `BlockAlreadyRendered` and `std::process::exit(1)` silently. Already wired into `main`. Reuse for `cmd_init`'s dependency-probe fatal-miss path in Task 17.
- **Three theme tiers, detection order:** TERM=dumb or non-TTY → `Ascii` → NO_COLOR set → `Mono` → else `Color`. Tests stub `IsTty`/`EnvGet` via `detect_with`. Don't add `std::env::set_var` in tests — project rule.

## Resuming

When the user is ready to continue:

1. `cd /Users/molt/dev/rightclaw/.worktrees/init-wizard-brand-redesign`
2. Confirm HEAD: `git rev-parse HEAD` should show `0d627dff` (or later if more landed during compaction).
3. Re-read the plan section "Step 4" and the spec section "Per-command flows · `right init`".
4. Resume by dispatching Task 16 (failing integration tests for `init` splash + recap) bundled with Tasks 17–19 (the cmd_init structural rewrite). Single commit.
5. Then Tasks 20–21 (voice rewrites in `init.rs` and `wizard.rs`) as a separate dispatch.

Recommended dispatch order for remaining steps:

| Dispatch | Tasks | Reason |
|---|---|---|
| 1 | 16 + 17 + 18 + 19 | Same `cmd_init` function, single test→commit cycle |
| 2 | 20 | `init.rs` prompt copy rewrites |
| 3 | 21 | `wizard.rs` tunnel/telegram/chat-id rewrites + rail status calls |
| 4 | 22 + 23 | `cmd_agent_init` recap + integration test |
| 5 | 24 + 25 | `wizard.rs` settings menus + sandbox/STT/memory copy |
| 6 | 26 | `PROMPT_LABELS` const + voice regression tests |
| 7 | 27 | Validation re-prompt warn lines |
| 8 | 28 + 29 + 30 + 31 | `--no-color` flag + brand-conformance lint + final verification |

Each dispatch ends with `cargo test --workspace --no-fail-fast && cargo clippy --workspace --all-targets -- -D warnings` clean.

## Pending follow-ups (low-priority, after main work)

- Bot crate's `/doctor` Telegram message now contains `▐` and `[ok]` characters — visually fine in Telegram but verify after manual smoke that it doesn't trigger any Markdown weirdness.
- The doctor_renders_brand_block_ascii test was renamed from a misleading `_in_mono` name. Same theme-name caveat applies to any new integration tests added in later steps.

## Outstanding user decisions

None at the time of this handoff — Steps 1–3 were authorized and shipped. Steps 4–6 are next; user previously offered three options (push through Step 4 / pause to manually verify / switch to inline batched execution). They asked for this handoff before deciding.
