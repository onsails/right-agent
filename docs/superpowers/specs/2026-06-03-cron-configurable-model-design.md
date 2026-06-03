# MCP-configurable per-cron model

**Date:** 2026-06-03
**Status:** Approved design

## Problem

Cron jobs snapshot the agent's **global** `/model` setting at fire time
(`snapshot_model(model)` in `crates/bot/src/cron.rs`) and pass it as
`--model` to `ClaudeInvocation`. There is no way to run a specific cron on a
cheaper/faster model. Most crons are mechanical and do not need the agent's
strongest model: routine health checks and summaries fit Sonnet, and trivial
"fetch a thing, format it, report" jobs fit Haiku. Running all crons on an
Opus global wastes budget and latency.

## Goal

Let the **session that creates the cron** pick the model for that cron, based
on its own judgment of the cron's complexity, via an MCP parameter. The
runtime never classifies complexity — it passes the chosen model through. The
`right-cron` skill teaches the heuristic so the creating agent chooses well.

Non-goal: runtime auto-selection of the model. That would clash with the
project's "debuggability over heuristics" convention and the
`feedback_no_self_classify_staleness` stance (show the signal, let the model
judge). The creating session decides; the runtime obeys.

## Selected approach (A): tier enum, bare alias passthrough

`model` is a 3-value tier enum (`haiku` | `sonnet` | `opus`) at the MCP param
boundary, passed as the **bare alias** straight to `--model` (Claude Code
resolves the aliases natively). Stored as nullable `TEXT`; omitted = inherit
the agent's global `/model` (backward-compatible).

Rejected alternatives:

- **B — free-form model string.** Maximum flexibility (could pin
  `claude-sonnet-4-6[1m]` or exact IDs) but the schema gives the agent no
  guidance and accepts garbage; all guidance would live only in the skill.
- **C — tier enum mapped to platform-pinned full IDs.** Consistency with
  `/model`'s exact pins, but duplicates the version constants
  (`claude-sonnet-4-6`, …) into another crate — exactly the drift the project
  warns against — and needs a shared knobs location.

Approach A keeps each crate's model registry local (the curated
`MODEL_CHOICES` table stays private to `crates/bot/src/telegram/model_command.rs`
per `feedback_no_central_registries`), carries zero version-pin maintenance,
and is self-documenting in the tool schema. Trade-off: the exact model floats
with CC's notion of each tier rather than being pinned to the platform-curated
version — acceptable for cost/latency-driven crons.

## Design

### 1. MCP param surface — `crates/right/src/memory_server.rs`

- New **local** enum, kept in this module (not a shared registry):

  ```rust
  #[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
  #[serde(rename_all = "lowercase")]
  pub enum CronModel { Haiku, Sonnet, Opus }

  impl CronModel {
      pub fn as_alias(&self) -> &'static str {
          match self { Self::Haiku => "haiku", Self::Sonnet => "sonnet", Self::Opus => "opus" }
      }
  }
  ```

- `CronCreateParams.model: Option<CronModel>`. The `#[schemars(description=...)]`
  is **concise** (1–2 sentences) because the tool schema ships to the agent on
  every turn — the full heuristic lives in the skill, not the schema.
- `CronUpdateParams.model: Option<Option<CronModel>>` via a double-option
  deserializer, mirroring the existing `target_thread_id` pattern
  (`deserialize_double_option_*`):
  - field **omitted** → leave unchanged
  - explicit **`null`** → clear back to inherit-global
  - a value → set that tier

### 2. Spec / persist / load — `crates/right-agent/src/cron_spec.rs`

- `CronSpec.model: Option<String>` — the resolved alias string, or `None` =
  inherit. The enum-ness stays at the MCP boundary in crate `right`; the
  validated alias string flows into `right-agent` as a plain string (the
  create/persist functions gain a `model: Option<&str>` argument).
- `PartialEq` for `CronSpec` adds `&& self.model == other.model`. The model is
  job configuration (like `target_chat_id`), so the reconciler must react to a
  `cron_update` that changes it. (It is NOT transient state like
  `triggered_at`.)
- Create-insert paths add `model` to the column list / bound params.
- The dynamic `cron_update` SET-builder adds `model = ?` when the field is
  present: a tier value sets the column, an explicit clear writes `NULL`.
- `load_specs_from_db` SELECT adds the `model` column and populates
  `CronSpec.model`.
- The `cron_list` JSON output (the listing SELECT) includes `model` so the
  agent and user can see each job's tier.

### 3. Migration — `crates/right-db/src/migrations.rs`

- New versioned migration appended to `MIGRATIONS`:
  `ALTER TABLE cron_specs ADD COLUMN model TEXT`, guarded by `column_exists`
  (Rust migration hook), idempotent.
- Nullable column → `NULL` = inherit global = today's behavior
  (backward-compatible). Existing agents adopt on `right restart` via the
  startup schema bootstrap. **No sandbox recreation.** The migration registry
  is the single add point (per the Local Database Rules).

### 4. Execute-time resolution — `crates/bot/src/cron.rs`

- New helper:

  ```rust
  fn resolve_cron_model(
      spec: &CronSpec,
      global: &arc_swap::ArcSwap<Option<String>>,
  ) -> Option<String> {
      spec.model.clone().or_else(|| crate::snapshot_model(global))
  }
  ```

- Applied at every `execute_job` call site (the trigger path and the
  per-job loop path) in place of the current bare `snapshot_model(model)`.
  Spec model wins; otherwise the global snapshot; otherwise `None` (CC default).
- The resolved value feeds both `ClaudeInvocation.model` and the learning
  `probe_writer_model_fallback`, so a Haiku cron's probe-writer fallback is
  also Haiku (minor consistency win; the learning pipeline's own
  `probe_writer_model` override still takes precedence where set).

### 5. Skill guidance — `crates/right-codegen/skills/right-cron/SKILL.md`

- Add a "Choosing the Model" section and a `model` row to the Parameters
  table. Bump `version` 3.4.0 → 3.5.0.
- Heuristic the creating session applies:
  - **`haiku`** — trivial: one request/tool call + mechanical formatting of the
    result (fetch → extract → report). No reasoning, no multi-step decisions.
  - **`sonnet`** — mechanical multi-step with light judgment: health checks,
    summaries, scheduled briefings, status polls. **Default for most crons.**
  - **`opus`** — genuinely complex: multi-source research, nuanced analysis,
    anything you'd want your strongest model for.
  - Omit `model` only to deliberately track the agent's current `/model`;
    otherwise set it explicitly by complexity.

### 6. Docs

- `PROMPT_SYSTEM.md`: update if/where it enumerates the cron `cron_create`
  JSON schema or cron params — convention requires keeping it in sync when a
  JSON schema changes.
- `ARCHITECTURE.md`: **no change** — a new optional param is routine feature
  evolution, not a new invariant/contract. Cite-on-touch: update the cron
  narrative in `docs/architecture/sessions.md` if it lists `CronSpec` fields.

## Backward compatibility & upgrade

- New nullable column; omitted `model` = inherit global = previous behavior.
- Existing running agents adopt the feature with no manual steps and no
  sandbox recreation: `right restart <agent>` runs the migration during schema
  bootstrap, and the new MCP param/skill ship on bot start. Fits the
  Upgrade & Migration Model (`Regenerated(BotRestart)` for the skill/schema;
  the DB column via the migration registry).

## Testing

Targeted per-crate tests during development; **one** full
`devenv shell -- cargo test --workspace` at the end (mandatory).

- `cron_spec` (crate `right-agent`):
  - persist + `load_specs_from_db` round-trips `model` (set and `None`).
  - `CronSpec` `PartialEq` returns false when only `model` differs (reconciler
    reacts).
  - `cron_update` sets a model, and a double-option clear writes `NULL`
    (back to inherit).
  - `cron_list` output includes `model`.
- migration (crate `right-db`): idempotency / column-added, via the existing
  migration test harness pattern.
- MCP params (crate `right`): `cron_create` with each tier enum deserializes
  and stores the alias; `cron_update` omit-vs-null-vs-value behaves per the
  double-option contract.
- `cron.rs` (crate `bot`): `resolve_cron_model` precedence — spec model over
  global; `None` falls back to the global snapshot. Extend the existing
  `cron_reads_current_model_from_arcswap` test.

## Out of scope (YAGNI)

- Per-run recorded model in `async_runs` (derivable from the spec).
- One-off model override on `cron_trigger` (it runs the stored spec).
- Reusing/centralizing `MODEL_CHOICES` across crates.
- `[1m]` context variants for crons (tier is what matters for cron workloads).
