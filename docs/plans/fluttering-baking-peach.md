# Memory pane in `agent config` (+ polish init wizard)

## Context

`rightclaw agent config` exposes only: Telegram token, Model, Allowed chat IDs,
Sandbox mode, Network policy (see `crates/rightclaw-cli/src/wizard.rs:584-597`).
But `MemoryConfig` (`crates/rightclaw/src/agent/types.rs:158`) supports
`provider` (file/hindsight), `api_key`, `bank_id`, `recall_budget`,
`recall_max_tokens` — none of which are editable via CLI after init. Users have
to hand-edit `agent.yaml`.

Project convention (`CLAUDE.md`): *"`agent config` must expose all user-facing
settings — if a feature exists but can't be toggled via CLI, it's incomplete."*
This plan closes that gap.

Additionally, the existing `init` memory wizard (`prompt_memory_config` in
`crates/rightclaw/src/init.rs:384`) doesn't validate the API key, doesn't
ask for `recall_budget` / `recall_max_tokens`, and doesn't warn on provider
switch. We unify the wizard code and add validation.

## Design decisions

- **Validation endpoint**: `GET /v1/default/banks` — read-only, requires auth,
  zero side effects. Confirmed via Hindsight API docs
  (https://hindsight.vectorize.io/api-reference). The existing
  `get_or_create_bank()` uses `GET /v1/default/banks/{id}/profile` which is
  **deprecated** + **auto-creates banks**, so we do NOT reuse it for validation.
  (Replacing `get_or_create_bank` itself is out of scope for this plan.)
- **Validation is advisory, not blocking**: 401 → warn + ask "save anyway?";
  5xx/timeout → warn + proceed. Never hard-fail the wizard on network issues.
- **API key storage**: continues to go to `memory.api_key` in `agent.yaml`
  (same as `telegram_token`). Agent dir lives in `~/.rightclaw/`, not project
  git, so this is consistent with existing patterns.
- **Env var fallback**: if `HINDSIGHT_API_KEY` is set, wizard offers "use env
  var [Y] / enter manually" — matches current bot resolution order
  (`crates/bot/src/lib.rs:164`).
- **Provider switch warning**: file ↔ hindsight does not migrate memory;
  wizard shows warning before confirming.

## Scope — files modified

1. **`crates/rightclaw/src/memory/hindsight.rs`**
   - Add `pub async fn list_banks(&self) -> Result<Vec<BankProfile>, MemoryError>`
     that calls `GET /v1/default/banks`.
   - Define response type: `#[derive(Deserialize)] struct ListBanksResponse { banks: Vec<BankProfile> }`.
   - Timeout: 5s (same as `RECALL_TIMEOUT`).
   - Tests: happy path (200 with banks / 200 empty), 401 (auth error), 500 (transient).

2. **`crates/rightclaw/src/init.rs`**
   - Extend `prompt_memory_config` signature to also return `recall_budget` and
     `recall_max_tokens` — new return type `(MemoryProvider, Option<String> /*key*/, Option<String> /*bank*/, RecallBudget, u32)`.
   - Add helpers: `prompt_recall_budget() -> miette::Result<Option<RecallBudget>>`,
     `prompt_recall_max_tokens() -> miette::Result<Option<u32>>`.
   - Add `pub async fn validate_hindsight_key(api_key: &str) -> ValidationResult`
     that constructs a `HindsightClient` (dummy bank_id, any budget — we only
     call `list_banks`), calls it, and returns `Valid | Invalid | Unreachable`.
     Returned from the helper so wizard can print a message and decide.
   - Update `init_rightclaw_home` / `init_agent` signatures to accept
     `recall_budget: RecallBudget`, `recall_max_tokens: u32` and emit them into
     the generated `agent.yaml` memory block (currently at
     `crates/rightclaw/src/init.rs:146`). Omit fields when equal to defaults
     (keeps yaml tidy).

3. **`crates/rightclaw-cli/src/wizard.rs`**
   - Add Memory option to the selection list around line 584. Display format:
     - `Memory: file` when provider=File
     - `Memory: hindsight (bank: X, budget: mid)` when provider=Hindsight
   - On select → call a new `memory_setup(current: &Option<MemoryConfig>, agent_name: &str) -> miette::Result<Option<MemoryConfig>>` submenu that:
     1. Picks provider (using `prompt_memory_provider` from init.rs).
     2. If switching away from current provider → `inquire::Confirm` warning
        about no migration.
     3. If Hindsight → reuse `prompt_hindsight_api_key`,
        `prompt_hindsight_bank_id`, `prompt_recall_budget`,
        `prompt_recall_max_tokens` from init.rs.
     4. Validate via `validate_hindsight_key` — print result, ask to save if Invalid.
   - Add `fn update_agent_yaml_memory(path: &Path, cfg: &MemoryConfig) -> miette::Result<()>`
     modelled on `update_agent_yaml_sandbox_mode` (line 755): remove existing
     `memory:` block (header + indented lines), append fresh block. Emit only
     non-default fields to keep yaml minimal. Also add `remove_agent_yaml_memory`
     for the `file + no overrides` case (clean block removal).
   - Update `agent init` wizard call site in `crates/rightclaw-cli/src/main.rs:1223`
     to pass the full 5-tuple through (budget/max_tokens added).

4. **`crates/rightclaw-cli/src/main.rs`**
   - Extend `memory_*` local bindings (line 1102-1123, 1146-1150) to include
     `recall_budget` + `recall_max_tokens`.
   - Update `init_rightclaw_home` call at line 1249 with new args.
   - Mirror same changes in `agent init <name>` path (around lines 1493-1644
     where `memory_api_key`/`memory_bank_id` already flow).
   - Non-interactive path: defaults = `RecallBudget::Mid`, `4096`.

## Reuse

- `prompt_memory_provider`, `prompt_hindsight_api_key`, `prompt_hindsight_bank_id`
  already exist in `crates/rightclaw/src/init.rs` (used by `init` wizard) — reuse
  from `agent config` wizard too. Move them to `pub` if not already.
- YAML write pattern from `update_agent_yaml_sandbox_mode`
  (`crates/rightclaw-cli/src/wizard.rs:755`) — identical shape works for
  `memory:` block.
- `HindsightClient::new` signature already supports construction with any
  `bank_id` — `list_banks` ignores it (path doesn't include bank_id).

## Verification

1. **Unit tests**
   - `hindsight::list_banks`: happy path (200, non-empty + empty), 401, 500.
     Use existing `mock_hindsight_server` pattern (line 302).
   - `init::validate_hindsight_key`: uses mock server, covers all 3 outcomes.
   - `wizard::update_agent_yaml_memory` round-trip: write provider=Hindsight with
     all fields, parse via `serde_saphyr::from_str::<AgentConfig>`, assert equal.
     Also: write File (provider only) → no `api_key`/`bank_id` leaked. Also: add
     block to yaml that had no `memory:` section → section appended cleanly.

2. **Build / lint**
   - `devenv shell -- cargo build --workspace`
   - `devenv shell -- cargo clippy --workspace -- -D warnings`

3. **Manual end-to-end**
   - `cargo run --bin rightclaw -- agent config him` →
     - "Memory: file" appears in menu.
     - Switching to Hindsight prompts api_key/bank_id/budget/max_tokens.
     - With a bogus key → "Key rejected. Save anyway? [y/N]".
     - With valid `$HINDSIGHT_API_KEY` → "Use env var? [Y]" path works.
     - After save, `cat ~/.rightclaw/agents/him/agent.yaml` shows correct
       `memory:` block.
     - Switching back to File → warning shown, `memory.api_key`/`bank_id` stripped.
   - `cargo run --bin rightclaw -- up --agents him` starts successfully with
     new config; bot log shows `"Hindsight bank ready"` when provider=Hindsight.

## Out of scope

- Replacing deprecated `get_or_create_bank()` with `list_banks` + targeted bank
  resolution. Separate plan — requires thinking about bank creation semantics.
- Hot-reload of memory config changes. Currently `config_watcher` restarts the
  bot on `agent.yaml` change; memory init happens at bot startup, so
  restart-to-apply is already correct. No extra work needed.
