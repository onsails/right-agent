# Decouple agent-facing prose constants from `right-core`

You are picking this up cold. Goal: stop a one-line tweak of agent-facing
copy/numbers from cascade-rebuilding the entire workspace.

## Problem

`right-core` is the bottom-of-stack crate; everything depends on it. Per
`ARCHITECTURE.md`, its modules "change rarely; leaf-crate edits do not
invalidate this build cache." That contract holds for `proto`, `openshell`,
`ui`, `error`, `config`, `platform_store`, `stt`, `test_support` — they
genuinely change rarely.

It does **not** hold for `crates/right-core/src/time_constants.rs`. That
file holds `IDLE_THRESHOLD_SECS` / `IDLE_THRESHOLD_MIN`, which gate
agent-facing prose in two places:

- `crates/right-codegen/src/skills.rs` (minijinja `{{ idle_threshold_min }}`
  substitution for `rightcron/SKILL.md`).
- `crates/right-agent/src/cron_spec.rs` (`TRIGGER_TOOL_DESC` built via
  `const_format::formatcp!`).
- `crates/bot/src/cron_delivery.rs` (runtime threshold check — this is the
  only actual runtime consumer).
- `crates/right/src/memory_server.rs` (literal `"...idle for N minutes..."`
  in a `#[tool(description = ...)]` attribute, pinned to `TRIGGER_TOOL_DESC`
  by a drift test).
- `crates/right-codegen/templates/right/prompt/OPERATING_INSTRUCTIONS.md`
  (hardcoded literal, pinned to the constant by a drift test added in
  `agent_def_tests.rs`).

Changing the number forces a full-workspace rebuild because everything
depends on `right-core`. UX-tunable knobs should not live in the most
expensive-to-invalidate crate.

## Goal

Extract `time_constants` into its own tiny leaf-level crate so that:

- Editing the value triggers a rebuild of the new crate + only its direct
  consumers (`right-codegen`, `right-agent`, `right-bot`), not `right-db`,
  `right-mcp`, `right-memory`, or anything else in `right-core`'s big graph.
- `right-core` returns to its "rarely changes" contract.
- All existing consumers continue to compile and pass tests with no
  behavioural change.

Out of scope: tuning the constant value, changing the prose, adding new
constants, or redesigning the delivery gate.

## Suggested approach

Create a new crate `crates/right-platform-knobs/` (name it whatever fits
your taste — `right-tunables`, `right-prose-constants`, `right-timings`).
Move `time_constants.rs` there. Update consumers:

- `right-codegen/Cargo.toml`: add the new crate. Update the import in
  `src/skills.rs` from `right_core::time_constants::*` to the new path.
- `right-agent/Cargo.toml`: add it. Update `src/cron_spec.rs` import + the
  `pub use` re-export.
- `right-bot/Cargo.toml`: add it. Update `src/cron_delivery.rs` import.
- `right-core`: delete `src/time_constants.rs` and its `pub mod
  time_constants;` line in `lib.rs`. Verify no other consumers inside
  `right-core` (there shouldn't be).
- `right-codegen/src/agent_def_tests.rs`: the new drift test
  `operating_instructions_cron_idle_threshold_matches_const` references
  `right_core::time_constants::IDLE_THRESHOLD_MIN`. Repoint it.

The new crate has no deps beyond `std`. Its `Cargo.toml` is ~10 lines.
Lift the existing module doc comment verbatim (it explains why the
constant exists — that explanation is the load-bearing piece, not the
number).

## Watch out

- `right-agent::cron_spec` does `pub use right_core::time_constants::{...}`
  to provide a stable import path for downstream. Preserve that
  re-export from the new crate so external callers in `right-agent`'s
  surface stay source-compatible (search for
  `right_agent::cron_spec::IDLE_THRESHOLD` to confirm consumers).
- `const_format::formatcp!` in `cron_spec.rs` interpolates
  `IDLE_THRESHOLD_MIN` at compile time. After the move, `const_format`
  must still be a dep of `right-agent` (it already is — just don't drop
  the dependency by accident).
- `ARCHITECTURE.md` currently says "Two constants migrate here" /
  "right-core hosts ... `IDLE_THRESHOLD_*` constants implicitly under
  `time_constants`" — neither phrasing is in the live doc as of now,
  but multiple superpowers `docs/superpowers/specs/` and
  `docs/superpowers/plans/` reference them in right-core. Spec/plan docs
  are historical and should NOT be rewritten; just update `ARCHITECTURE.md`
  and `docs/architecture/sessions.md` (line 45 mentions `IDLE_THRESHOLD_SECS
  = 120` — keep the number but you may want to add a one-liner saying
  the constants moved out of right-core).
- New crate must appear in workspace members in root `Cargo.toml`.
- `right-core` may have its `[features]` section reference time_constants
  — unlikely, but check. The `test-support` feature is unrelated.
- Per `AGENTS.rust.md`: edition 2024, error types via `thiserror`/`anyhow`
  rules (irrelevant here — pure constants, no error paths).

## Verification

```bash
devenv shell -- cargo build --workspace
devenv shell -- cargo test --workspace
```

Then prove the cascade is broken: change `IDLE_THRESHOLD_SECS` value by
one and run `cargo build --workspace -v 2>&1 | rg -i "Compiling (right-|right_)"`.
The list should contain the new crate + `right-codegen` + `right-agent` +
`right-bot` (and their test/binary variants). It should NOT contain
`right-db`, `right-mcp`, `right-memory`, or `right-core`. Revert the value
change after verifying.

Also confirm:

- `cargo test -p right-codegen` passes (the OPERATING_INSTRUCTIONS drift
  test still finds the constant).
- `cargo test -p right` passes (`cron_trigger_description_matches_const`
  still works — the constant moved, the test path needs to be repointed
  if it accessed `right_agent::cron_spec::TRIGGER_TOOL_DESC` directly).

## Commit

Single commit. Conventional title:

```
refactor(workspace): extract time_constants into right-platform-knobs
```

Body: one paragraph explaining the cascade-rebuild motivation. Reference
this prompt file or the prior conversation for context.

Do not push.
