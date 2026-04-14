# Home Directory in System Prompt

## Problem

CC agent inside OpenShell sandbox doesn't know its working directory. BOOTSTRAP.md says "Write all three files in your current working directory" but CC guesses `/root/` (standard Linux root home), fails, then recovers after running `pwd`. Wastes turns and confuses the agent.

Root cause: system prompt never tells the agent its home/working directory explicitly. Both sandbox (`HOME=/sandbox`) and no-sandbox (`HOME={agent_dir}`) set `$HOME` correctly, but CC's Write tool uses absolute paths and the LLM has to guess.

Secondary: `OPERATING_INSTRUCTIONS.md` hardcodes `/sandbox/outbox/` for attachments, which is wrong for no-sandbox mode.

## Design

### Approach: Pass home_dir to generate_system_prompt() (B1)

Add `home_dir: &str` parameter to `generate_system_prompt()`. Include it in the `## Environment` section of the base system prompt. All agent-facing text references "your home directory" — the agent sees the concrete path in the environment block and uses it.

Prompt caching is not affected: each agent already has a unique prompt (agent_name differs), and home_dir is stable across sessions for the same agent.

### Changes

#### 1. `crates/rightclaw/src/codegen/agent_def.rs`

`generate_system_prompt(agent_name, sandbox_mode)` → `generate_system_prompt(agent_name, sandbox_mode, home_dir)`

Add to `## Environment` section:
```
- Home / working directory: {home_dir}
```

#### 2. `templates/right/agent/BOOTSTRAP.md`

Replace:
```
Write all three files in your current working directory using the Write tool.
Do NOT create them inside `.claude/`, `.claude/agents/`, or any subdirectory.
```

With:
```
Write all three files in your home directory using the Write tool.
Do NOT create them inside `.claude/`, `.claude/agents/`, or any subdirectory.
```

#### 3. `templates/right/prompt/OPERATING_INSTRUCTIONS.md`

After line 92 (`Use the Read tool to view images and files at the given paths.`), add:
```
Attachments are downloaded to the inbox/ directory in your home directory.
```

Line 96, replace:
```
Write files to /sandbox/outbox/ (or the outbox/ directory in your working directory).
```
With:
```
Write files to the outbox/ directory in your home directory.
```

#### 4. Callers

| File | Line | home_dir value |
|------|------|----------------|
| `crates/bot/src/telegram/worker.rs` | 856 | `"/sandbox"` when `ctx.ssh_config_path.is_some()`, else `ctx.agent_dir.to_string_lossy()` |
| `crates/rightclaw/src/codegen/pipeline.rs` | 96 | `"/sandbox"` when `agent_sandbox_mode == Openshell`, else `agent.path.to_string_lossy()`. Pipeline writes `system-prompt.md` to platform store — sandbox agents see `/sandbox`, no-sandbox agents see their host agent_dir. |
| `crates/rightclaw-cli/src/main.rs` | 2157 | `agent.path.to_string_lossy()` — `cmd_pair` runs on host in no-sandbox context |

#### 5. Tests

`crates/rightclaw/src/codegen/agent_def_tests.rs` — 8 existing calls to `generate_system_prompt()` need a third `home_dir` argument (use `"/sandbox"` or `"/test/home"`). Add one new test: call with `home_dir = "/my/home"`, assert output contains `"/my/home"`.

### Not in scope

- `bootstrap_done` checking files on host instead of in sandbox — separate bug, separate fix.
- Reverse sync timing — separate concern.

## Verification

1. All existing tests pass with updated signatures
2. New test: `generate_system_prompt` output contains the passed home_dir
3. Manual: bootstrap session writes IDENTITY.md to correct path on first attempt (no `/root/` detour)
