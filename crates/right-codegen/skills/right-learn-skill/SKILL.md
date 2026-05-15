---
name: right-learn-skill
description: >-
  Create or update reusable Agent Skills from real work. Use when a workflow,
  recovered surprise, user correction, or loaded non-core skill issue should be
  saved as a skill package for future sessions.
version: 0.1.0
compatibility: Uses standard Claude Code Agent Skills in .claude/skills.
---

# /right-learn-skill -- Learn Or Update Skills

Use this skill only when the lesson is reusable across future sessions.

## Create A New Skill

Create a new skill when at least one trigger is true:

- The user explicitly asked you to learn, save, or remember the workflow.
- The task required several non-obvious repeated steps.
- A command, tool, API, or MCP call failed or returned an unexpected shape, and you found a verified reusable path.
- The user corrected your approach and the correction is a durable gotcha.
- You discovered a repeated tool/API usage pattern likely to recur.

New skills created by Right learning must use an `rl-` package name:

```text
.claude/skills/rl-<slug>/SKILL.md
```

Use lowercase ASCII letters, digits, and hyphens. Do not use absolute paths.

## Update An Existing Skill

Update an existing non-core skill when a loaded skill was materially wrong or incomplete:

- missing required step
- stale command or API behavior
- wrong API assumption
- overbroad activation
- broken script
- unsafe instruction

You may update custom, manually installed, hub-installed, and `rl-*` learned skills.
Do not update core/platform/bundled/codegen-owned skills.

## Skip

Do not create a skill for one-off task details, temporary project progress, generic memory facts, unverified workarounds, or failed attempts without a verified path.

## Required Protocol

Before writing or patching any skill package file, call:

```text
mcp__right__skill_learning_start
```

Use `action: "create"` for new `rl-*` skills and `action: "update"` for existing non-core skills. Include a short localized message that tells the user what is being learned or updated.

Only write or patch skill files after the start call succeeds. If the start call is rejected or unavailable, do not write or patch package files; report the rejection or defer learning until the protocol is available.

Do not call mcp__right__send_progress just to announce learning. The learning start tool sends the user-visible progress message.

After the write succeeds or fails, call:

```text
mcp__right__skill_learning_finish
```

Use `status: "created"` or `status: "updated"` only after the package files are written. Successful `created`/`updated` calls must include an LLM-authored receipt message in the `message` argument; this is the user-visible learned/updated receipt. Use `status: "failed"` or `status: "aborted"` when the write did not complete.

Successful finish calls send the learned/updated receipt. Failure finish calls record evidence and do not send a success receipt.

## Package Shape

Use the full Agent Skills format:

```text
.claude/skills/<skill_name>/
  SKILL.md
  scripts/
  references/
  assets/
```

Include `scripts/`, `references/`, or `assets/` only when they remove real complexity from future use.

Update `.claude/skills/installed.json` for new learned skills. Read the existing file first and treat a missing file as `{}`. Preserve all existing installed.json entries, write a valid object entry for the new learned skill, and never rewrite or delete unrelated registry data. The entry must use `source: "learned"` and `path: ".claude/skills/rl-<slug>"`.

```json
{
  "rl-<slug>": {
    "source": "learned",
    "path": ".claude/skills/rl-<slug>"
  }
}
```

## Skill Quality

Write `description` so the skill loads only for the right future tasks. Prefer concrete triggers over broad categories.

In `SKILL.md`, include:

- when to use the skill
- exact steps that worked
- tool/API gotchas
- verification command or success check
- when not to use it

Do not store secrets. Do not copy large transcripts. Keep references focused.

## Deferred Signal

If the conversation is still evolving or a full-context review is safer, do not write a half-baked skill. Instead, leave one hidden structured output signal:

- `learning_signal` for a new skill candidate
- `skill_issue_signal` for an existing non-core skill problem

Emit no signal after a successful `mcp__right__skill_learning_finish`.
