# Add Memory Provider to `agent config` and `agent init` Wizards

## Context

Hindsight memory integration landed on the `hindsight` branch, but there's no way to enable it on an already-deployed agent. `rightclaw agent config` doesn't show a memory option, and `agent init` hardcodes `MemoryProvider::File`. This violates the upgrade-friendly design principle: every feature must be adoptable by running agents without recreation.

Additionally, `RESTRICTIVE_DOMAINS` in policy.rs doesn't include `*.vectorize.io`, so Hindsight API calls would fail from sandboxed agents with restrictive network policy.

## Changes

### 1. `crates/rightclaw/src/codegen/policy.rs` — Add vectorize.io to restrictive domains

Always include Hindsight API domains in restrictive policy (no conditional logic).

- Add `"*.vectorize.io"` and `"vectorize.io"` to `RESTRICTIVE_DOMAINS` array (~line 6)
- Update test `restrictive_policy_allows_only_anthropic_domains` to assert vectorize domains

### 2. `crates/rightclaw-cli/src/wizard.rs` — Memory setup function + YAML helper + menu option

**New function `memory_setup()`** (~after `chat_ids_setup`):
- `pub fn memory_setup(agent_name: &str, current: Option<&MemoryConfig>) -> miette::Result<(MemoryProvider, Option<String>, Option<String>)>`
- Select: "File — MEMORY.md managed by agent (default)" / "Hindsight — cloud memory via Hindsight API"
- If File → return `(File, None, None)`
- If Hindsight:
  - Text prompt for API key (show current masked if exists, mention HINDSIGHT_API_KEY env var alternative, allow empty)
  - Text prompt for bank_id (default = agent name, Enter to keep)
  - Return `(Hindsight, api_key, bank_id)`

**New YAML helper `update_agent_yaml_memory()`** (~after `update_agent_yaml_sandbox_mode`):
- Pattern: same as `update_agent_yaml_sandbox_mode` — remove existing `memory:` block + indented lines, append new block
- If File → remove `memory:` block entirely (it's the default, no need in yaml)
- If Hindsight → write `memory:\n  provider: hindsight\n  api_key: "..."\n  bank_id: "..."`
- Skip `api_key` line if None (user will use env var)
- Skip `bank_id` line if None (defaults to agent name)

**Menu option in `agent_setting_menu()`** (~line 563):
- Add `opt_memory` display: `Memory: file` or `Memory: hindsight (bank: <id>)`
- Read memory config from parsed `config.memory`
- Insert before `opt_done` in options vec
- Handle selection: call `memory_setup()`, then `update_agent_yaml_memory()`

### 3. `crates/rightclaw-cli/src/main.rs` — Wire memory into `agent init`

In `cmd_agent_init` interactive block (~line 1112):
- After chat_ids setup, before building `InitOverrides`
- Call `crate::wizard::memory_setup(name, None)?` in interactive mode
- Pass results to `InitOverrides { memory_provider, memory_api_key, memory_bank_id }`
- Non-interactive (`--yes`): keep current default `MemoryProvider::File`

## Files to modify

| File | Change |
|------|--------|
| `crates/rightclaw/src/codegen/policy.rs:6` | Add vectorize.io to RESTRICTIVE_DOMAINS |
| `crates/rightclaw-cli/src/wizard.rs:502-665` | Add memory_setup(), update_agent_yaml_memory(), menu option |
| `crates/rightclaw-cli/src/main.rs:1112-1122` | Wire memory_setup() into agent init |

## Reuse

- `update_agent_yaml_sandbox_mode()` pattern for the YAML block helper (wizard.rs:732)
- `telegram_setup()` pattern for the setup function signature (wizard.rs:352)
- `MemoryConfig` and `MemoryProvider` from `crates/rightclaw/src/agent/types.rs:158-185`

## Verification

1. `cargo build --workspace` — clean build
2. `cargo test --workspace` — all tests pass (including updated policy test)
3. Manual: `cargo run --bin rightclaw -- agent config` → select agent → verify Memory option appears with correct current value
4. Manual: change Memory to Hindsight → verify agent.yaml has correct `memory:` block
5. Manual: change Memory back to File → verify `memory:` block removed from agent.yaml
6. Manual: `cargo run --bin rightclaw -- agent init test-agent` → verify memory prompt appears in wizard
7. Check generated policy.yaml contains vectorize.io domains
