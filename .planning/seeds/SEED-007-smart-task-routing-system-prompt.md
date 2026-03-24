---
id: SEED-007
status: dormant
planted: 2026-03-24
planted_during: v2.0 completion
trigger_when: next milestone
scope: Small
---

# SEED-007: Smart task routing in system prompt — model selection and background execution by complexity

## Why This Matters

Agents currently use a single model for everything and process all tasks inline. Three improvements:

1. **Background execution for hard tasks** — When an agent receives a complex task via Telegram (or other remote channel), it should spawn the work in background and send feedback through the channel when done. Currently agents block on hard tasks, leaving the channel unresponsive.

2. **Model routing by complexity** — Hard tasks route to Opus, moderate tasks to Sonnet, simple questions spawn a Haiku subagent for instant response. This reduces cost and latency for trivial interactions while maintaining quality for complex work.

3. **Guaranteed remote channel feedback** — When a task runs in background, the agent must send a completion/failure message back through the channel it received the task from. No silent drops.

## When to Surface

**Trigger:** Next milestone — this is a system prompt template improvement, not a major feature.

This seed should be presented during `/gsd:new-milestone` when the milestone scope matches any of these conditions:
- Any new milestone (general improvement, always relevant)
- Agent autonomy or intelligence improvements
- Telegram/channel UX improvements
- Cost optimization or model management

## Scope Estimate

**Small** — Primarily system prompt template changes in `codegen/system_prompt.rs` and possibly `agent.yaml` model configuration. The routing logic lives in the prompt text (CC already supports `--model` and subagent spawning), not in Rust code.

## Breadcrumbs

Related code and decisions found in the current codebase:

- `crates/rightclaw/src/codegen/system_prompt.rs` — generates combined prompt (identity + start + optional bootstrap/rightcron)
- `crates/rightclaw/src/agent/types.rs` — `AgentConfig.model: Option<String>` already exists
- `templates/agent-wrapper.sh.j2` — passes `--model {{ model }}` when set
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — model flag wiring to wrapper template
- `templates/right/IDENTITY.md` — agent's self-description, "How you work" section

## Notes

- CC already supports `run_in_background` on Task tool — the system prompt just needs to instruct agents to use it for complex tasks
- Model routing could be prompt-level ("for simple questions, use a haiku subagent") or agent.yaml-level (multiple model tiers configured)
- Telegram channel feedback already works — the agent just needs instructions to always report back after background work completes
