---
name: right-learn-skill
description: >-
  Use when you verified a reusable procedure this turn (non-trivial multi-step
  work, concrete gotchas, or you just corrected a wrong approach) and want to
  save or fix a rightx-* skill, or when the user explicitly asks to save,
  remember, or fix one. Not for one-off tasks, unverified guesses, or trivial
  single-step work.
version: 0.3.0
compatibility: Uses standard Claude Code Agent Skills in .claude/skills.
---

# /right-learn-skill — Mid-Conversation Skill Writes

Create or fix a `rightx-*` learned skill, either:

- the user explicitly asks ("save this as a skill", "remember how to do X",
  "this skill is broken, fix it"), **or**
- on your own judgment, when *during this turn* you verified a reusable "how":
  non-trivial multi-step work, concrete gotchas / exact commands / API quirks,
  and you are confident it will recur. The strongest trigger is correcting a
  wrong approach and now knowing the right one.

The post-turn probe-writer is the safety net for turns where you did NOT
capture the "how" yourself; on any turn where you author or patch a skill, it
does not run. In a cron turn, only write a skill after your `delivery` output
is secured — the budget that produces the deliverable comes first.

## Create A New Skill

Create a new skill when the user explicitly asked you to learn, save, or
remember the workflow, or when you verified a reusable procedure this turn.

New skills created by Right learning must use a `rightx-` package name:

```text
.claude/skills/rightx-<slug>/SKILL.md
```

Use lowercase ASCII letters, digits, and hyphens. Do not use absolute paths.

## Update An Existing Skill

Update an existing `rightx-*` learned skill when it was materially wrong or incomplete:

- missing required step
- stale command or API behavior
- wrong API assumption
- overbroad activation
- broken script
- unsafe instruction

You may update only existing `rightx-*` learned skills. Do not update custom, manually installed, hub-installed, core/platform/bundled, or codegen-owned skills through this learning flow.

## Skip

Do not create a skill for one-off task details, temporary project progress, generic memory facts, unverified workarounds, or failed attempts without a verified path.

## Required Protocol

Before writing or patching any skill package file, call:

```text
mcp__right__skill_learning_start
```

Use `action: "create"` for new `rightx-*` skills and `action: "update"` for existing `rightx-*` skills. Include a short localized message that tells the user what is being learned or updated.

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

Update `.claude/skills/installed.json` for new learned skills. Read the existing file first and treat a missing file as `{}`. Preserve all existing installed.json entries, write a valid object entry for the new learned skill, and never rewrite or delete unrelated registry data. The entry must use `source: "learned"` and `path: ".claude/skills/rightx-<slug>"`.

```json
{
  "rightx-<slug>": {
    "source": "learned",
    "path": ".claude/skills/rightx-<slug>"
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
- that future use of this `rightx-*` learned skill should emit a short localized `used_skill_receipts` message when it materially guides the answer

When the procedure is multi-step with mechanical or disposable-intermediate
steps, encode concrete subagent-delegation directives in the steps, naming the
model tier (`haiku` for purely mechanical work like format conversion or field
extraction, `sonnet` for mechanical work needing light comprehension like long
reads or summarization). Keep simple single-procedure recipes free of
delegation directives.

Do not store secrets. Do not copy large transcripts. Keep references focused.

