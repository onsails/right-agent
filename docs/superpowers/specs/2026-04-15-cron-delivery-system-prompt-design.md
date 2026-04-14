# Cron & Delivery: System Prompt Alignment

## Problem

`cron.rs` (`execute_job`) and `cron_delivery.rs` (`deliver_through_session`) invoke `claude -p` without a system prompt. They pass `--agent <name>` which CC ignores (no `.claude/agents/` directory exists). Meanwhile `worker.rs` assembles a composite system prompt via `build_prompt_assembly_script()` with identity files, operating instructions, and MCP instructions. Result: cron jobs and delivery sessions run as bare CC with no agent identity, no MCP tool documentation, and no behavioral instructions.

## Changes

### 1. Extract prompt assembly to `telegram/prompt.rs`

Move from `worker.rs` to a new `telegram/prompt.rs` module:
- `build_prompt_assembly_script()` — unchanged signature
- `PROMPT_SECTIONS` constant
- `shell_escape()` helper

`worker.rs` re-imports from `telegram::prompt`. Cron and delivery import from the same place.

### 2. Thread new parameters through cron

`execute_job` and `run_cron_task` gain one new parameter:
- `internal_client: Arc<InternalClient>` — to fetch MCP instructions

`sandbox_mode` and `home_dir` are derived locally from `ssh_config_path` (same logic as worker: SSH → `Openshell` + `/sandbox`, direct → `None` + `agent_dir`).

`lib.rs` already has `internal_client` at the call site. Clone and pass through.

### 3. Thread new parameters through delivery

Same: add `internal_client: Arc<InternalClient>` to `run_delivery_loop` and `deliver_through_session`. Derive `sandbox_mode`/`home_dir` locally.

### 4. Remove `--agent`, add `--system-prompt-file`

In both `execute_job` and `deliver_through_session`:

1. Remove `--agent` and `agent_name` from `claude_args`
2. Call `generate_system_prompt(agent_name, &sandbox_mode, &home_dir)`
3. Fetch MCP instructions: `internal_client.mcp_instructions(agent_name).await`
4. Call `build_prompt_assembly_script(base_prompt, false, root_path, prompt_file, workdir, &claude_args, mcp_instructions)`
5. SSH path: the assembly script becomes the full SSH command (same as worker)
6. Direct path: `bash -c <assembly_script>` (same as worker)

Auth token injection stays as-is — prepended to the script (SSH) or via `cmd.env()` (direct).

### 5. Delivery uses Haiku model

`deliver_through_session` hardcodes `claude-haiku-4-5-20251001` as model, ignoring the `model` parameter from agent config. Delivery is a simple relay task — haiku is sufficient and cheaper.

In `run_delivery_loop`, the `model` parameter is replaced with the hardcoded haiku model string. The `model: Option<String>` parameter is removed from `run_delivery_loop` since delivery always uses haiku.

### 6. Schemas unchanged

- Cron: `CRON_SCHEMA_JSON` (structured output with summary + notify)
- Delivery: `reply-schema.json` (standard reply schema)

## Files modified

| File | Change |
|------|--------|
| `crates/bot/src/telegram/prompt.rs` | New module: extracted `build_prompt_assembly_script`, `PROMPT_SECTIONS`, `shell_escape` |
| `crates/bot/src/telegram/mod.rs` | Add `pub(crate) mod prompt;` |
| `crates/bot/src/telegram/worker.rs` | Remove moved functions, import from `telegram::prompt` |
| `crates/bot/src/cron.rs` | Remove `--agent`, add system prompt assembly, thread `internal_client`/`sandbox_mode`/`home_dir` |
| `crates/bot/src/cron_delivery.rs` | Remove `--agent`, add system prompt assembly, thread `internal_client`/`sandbox_mode`/`home_dir`, hardcode haiku |
| `crates/bot/src/lib.rs` | Pass `internal_client`, `sandbox_mode`, `home_dir` to cron and delivery |
| `PROMPT_SYSTEM.md` | Update: prompt assembly now shared across worker/cron/delivery |
| `ARCHITECTURE.md` | Update module map: new `telegram/prompt.rs`, cron/delivery now use system prompt |

## Not changed

- `build_prompt_assembly_script` signature — no modifications needed
- Output schemas — cron and delivery keep their respective schemas
- Retry cap and error reporting fixes from earlier in this session — orthogonal
