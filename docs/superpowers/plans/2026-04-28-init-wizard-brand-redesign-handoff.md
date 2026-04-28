# Init Wizard Brand Redesign — Handoff

**Read this first when resuming after compaction.**

## Locations

| What | Path |
|---|---|
| **Plan** | `docs/superpowers/plans/2026-04-28-init-wizard-brand-redesign.md` (31 tasks across 6 steps) |
| **Spec** | `docs/superpowers/specs/2026-04-28-init-wizard-brand-redesign-design.md` |
| **Worktree** | `.worktrees/init-wizard-brand-redesign/` |
| **Branch** | `feature/init-wizard-brand-redesign` (rebased on `origin/master`) |
| **This handoff** | `docs/superpowers/plans/2026-04-28-init-wizard-brand-redesign-handoff.md` |

`cd` into the worktree before resuming. All commands below assume that working directory.

## Execution mode

Subagent-driven (`superpowers:subagent-driven-development`). One implementer subagent per task (or batched group), two-stage review per task (spec compliance + code quality). Plan was created via `superpowers:writing-plans`.

## Progress

| Step | Tasks | Status |
|---|---|---|
| 1 | 1–9 | ✅ `right-agent::ui` module shipped (atoms, theme, line/block, splash, header, recap, writer, error sentinel) |
| 2 | 10–12 | ✅ `cmd_doctor` migrated to `ui::Block`; integration tests for doctor |
| 3 | 13–15 | ✅ `cmd_status` migrated; `BlockAlreadyRendered` wired into `main::handle_dispatch` |
| 4 | 16–21 | ✅ `right init` redesigned (splash + dep probe + section headers + recap + voice rewrites in `init.rs`/`wizard.rs`) |
| 5 | 22–27 | ✅ `cmd_agent_init` recap + integration test; `wizard.rs` settings menus / sandbox / STT / memory voice rewrites; `PROMPT_LABELS` const + voice regression tests; validation re-prompt warn lines |
| 6 | 28–31 | ⏳ **next**: `right --no-color` global flag + ASCII fallback integration tests + brand-conformance lint + final clippy/build |

**HEAD on resume:** `d5ca3f5c` (after the PROMPT_LABELS scope-expansion cleanup; rebased on origin/master).

## Recent commit chain (Step 5 + cleanups)

```
d5ca3f5c test(voice): cover Select options + lowercase 'use HINDSIGHT_API_KEY'   ← HEAD
6b6641e2 test(voice): lowercase + no-exclamation regression for prompt labels    (Task 26)
86cbb0f3 refactor(wizard): brand warn lines on validation re-prompt              (Task 27)
17fcb51e refactor(wizard): consolidate theme rebinds + diagnostic unreachable msg (Task 24-25 cleanup)
adc3977b refactor(wizard): lowercase memory/stt/sandbox copy + rail status       (Task 25)
842bfa54 refactor(wizard): lowercase settings menu copy + rail saved lines       (Task 24)
e7738362 refactor(agent-init): drop duplicate theme rebind; rename test          (Task 22-23 cleanup)
ba0fd93f test(agent-init): assert recap block on completion                      (Task 23)
d9f937db feat(agent-init): section header + recap                                (Task 22)
480f4ffb refactor(wizard): drop duplicate theme rebinds in DeleteAndRecreate     (Task 21 cleanup)
1c7b6172 refactor(wizard): lowercase tunnel/telegram/chat-id copy + rail status  (Task 21)
986d92ff refactor(init): lowercase-first prompt copy per brand                   (Task 20)
04d3b3d1 feat(init): recap block replaces footer                                 (Task 19)
39b0c7f5 feat(init): section headers + sandbox-creation status lines             (Task 18)
499483d4 feat(init): splash + dependency probe block                             (Tasks 16+17)
92286184 docs(handoff): pause after step 3 of brand redesign                     (prior handoff)
f512dd39 feat(status): brand-conformant rail+glyph block                         (Task 14)
… (Steps 1–3 commits omitted; see `git log master..HEAD`)
```

## Verification gates currently passing

```
cargo build --workspace                                    # clean
cargo clippy --workspace --all-targets -- -D warnings      # zero warnings
cargo test -p right --test wizard_brand                    # 7 tests pass
cargo test -p right-agent --test voice_pass                # 2 tests pass
cargo test -p right --bin right wizard::voice_pass         # 2 tests pass
cargo test -p right-agent ui:: --no-fail-fast              # ~46 unit tests pass
```

## Deviations from the plan (locked in)

1. **`IsTty` instead of `IsTerminal`.** `std::io::IsTerminal` is sealed in the active toolchain. We have `pub(crate) trait IsTty { fn is_tty(&self) -> bool; }` with `RealTty` delegating to `stdout().is_terminal()`.
2. **`Rail::prefix/mark/blank` return `String`, not `&'static str`.** The Color path produces ANSI-wrapped owned strings. Spec doc patched.
3. **`Mono` integration tests omitted.** `assert_cmd` runs the binary non-TTY → theme detection forces `Ascii` regardless of `NO_COLOR`. The `Mono` branch is fully covered by unit tests in `theme_tests.rs` / `splash_tests.rs` / `atoms_tests.rs` / `recap_tests.rs`. Integration tests use Ascii substring assertions.
4. **`init_rerun_writes_recap_again` test rewritten.** Plan said "two runs against the same home"; `init_right_home` rejects re-init without `--force`, so the test uses two independent homes instead — same property (recap appears on every fresh init).
5. **`Option<&str>` for `saved_noun`** in `agent_setting_menu` instead of plain `&str`. Cancel/skip/back paths return `None`, and the outer `if let Some(noun)` gates the saved-line print. This is a small UX improvement vs the original which fired `Saved.` even on cancel.
6. **`PROMPT_LABELS` covers labels AND options.** First implementer narrowed scope to Select-first-args; review caught the gap; cleanup commit (`d5ca3f5c`) expanded both arrays to include every Select option string. Doc comments in both files now read "every user-visible string from every prompt — labels, Select options, and static prefixes of dynamic-format prompts."
7. **Live source fix during Task 26.** The Task 25 implementer left `"Use HINDSIGHT_API_KEY env var (recommended)"` capitalized (treating it as env-var-led prose). Task 26's regression caught it; cleanup commit lowercased the option to `"use HINDSIGHT_API_KEY env var (recommended)"` and updated the matcher (`starts_with`).

## Critical pieces that must be reused

- **`handle_dispatch` helper in `crates/right/src/main.rs`** (top of file). Wraps the CLI dispatch result; intercepts `BlockAlreadyRendered` and `std::process::exit(1)` silently. Reuse for any future "I already rendered an error block; suppress miette" path.
- **Three theme tiers, detection order:** `TERM=dumb` or non-TTY → `Ascii` → `NO_COLOR` non-empty → `Mono` → else `Color`. Tests stub `IsTty`/`EnvGet` via `detect_with`. Don't add `std::env::set_var` in tests — project rule.
- **`PROMPT_LABELS`** is the source-of-truth for brand voice regression. Every `inquire::Select::new` first-arg AND `vec![...]` option string AND dynamic-format static prefix must appear there. The test in `crates/right-agent/tests/voice_pass.rs` and `crates/right/src/wizard.rs::voice_pass` enforces lowercase-first + no `!` (with `ALLOWED_PROPER_NOUNS` exception list).

## What remains: Step 6 (Tasks 28–31)

| Task | Summary |
|---|---|
| 28 | `right --no-color` global flag (`#[arg(long, global = true)]` on `Cli`; `unsafe { std::env::set_var("NO_COLOR", "1") }` early in `main`) |
| 29 | ASCII fallback + mono no-ANSI integration tests in `wizard_brand.rs` (mostly already covered in Tasks 12+16+22-23; Task 29 is gap-filling) |
| 30 | Brand-conformance lint — a separate test or compile-time check that scans source for raw `▐` / `✓` / `✗` / `…` outside the `right-agent::ui` module (catching regressions where someone bypasses the module) |
| 31 | Final workspace verification: `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` |

The plan's full text for these tasks is in `docs/superpowers/plans/2026-04-28-init-wizard-brand-redesign.md` lines 2898–3229.

## Resuming

When the user is ready to continue:

1. `cd /Users/molt/dev/rightclaw/.worktrees/init-wizard-brand-redesign`
2. Confirm HEAD: `git rev-parse HEAD` should show `d5ca3f5c` (or later if more landed during compaction).
3. Re-read the plan section "Step 6" and the spec sections referenced there.
4. Dispatch Tasks 28+29+30+31 — they're independent enough that 28 can be its own commit, 29 batched with 28 (both touch `wizard_brand.rs`), 30 standalone, 31 is just verification.

Recommended dispatch order:

| Dispatch | Tasks | Reason |
|---|---|---|
| 1 | 28 + 29 | `--no-color` flag + the integration tests that exercise it |
| 2 | 30 | Brand-conformance lint (separate concern) |
| 3 | 31 | Final verification (no code changes; just gates) |

Each dispatch ends with `cargo test --workspace --no-fail-fast && cargo clippy --workspace --all-targets -- -D warnings` clean.

## Pending follow-ups (low-priority, after main work)

- Bot crate's `/doctor` Telegram message contains `▐` and `[ok]` characters — visually fine in Telegram but verify after manual smoke that it doesn't trigger any Markdown weirdness.
- Some prompt-prefix entries in `PROMPT_LABELS` end with a trailing space (e.g. `"hindsight bank id (default: "`, `"agent: "`). They are correct as static format-string prefixes but look odd at a glance — consider documenting the convention in the doc comment if it surprises future readers.

## Outstanding user decisions

None at the time of this handoff. Steps 1–5 are shipped and reviewed. Step 6 (4 tasks remaining) is the only work left before `cargo test --workspace` is the final gate.
