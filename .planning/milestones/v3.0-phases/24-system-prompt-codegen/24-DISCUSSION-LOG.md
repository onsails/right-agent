# Phase 24: System Prompt Codegen - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.

**Date:** 2026-03-31
**Phase:** 24 — System Prompt Codegen

---

## Areas Discussed

All four gray areas selected by user.

---

### PC entries after wrapper removed

**Q:** After shell wrappers are removed, what should rightclaw up put in process-compose for CC sessions?

**Options presented:**
- Inline claude command (update PC template with environment: block)
- Leave PC broken until Phase 26 ← **Selected**
- Remove CC entries entirely

**Decision:** Leave PC broken until Phase 26. Phase 24 only removes shell_wrapper.rs and writes system-prompt.txt. CC sessions in PC will be stale/broken until Phase 26 replaces them with bot processes.

---

### New system-prompt.txt content

**Q1:** Which identity files should be concatenated?

**Options presented:**
- SOUL + USER + AGENTS only
- All 4: SOUL + USER + AGENTS + IDENTITY ← **Selected**
- Configurable list in agent.yaml

**User note:** "make sure there are ones which agent can edit and ones which it cannot edit — permissions must be handled correctly. Look OpenClaw's instructions."

**Decision:** All 4 OpenClaw files (IDENTITY, SOUL, USER, AGENTS). File permissions per OpenClaw convention (USER.md writable, others read-only).

---

**Q2:** What happens to old hardcoded sections (communication, cron, BOOTSTRAP.md detection)?

**Options presented:**
- Drop hardcoded sections ← **Selected**
- Keep hardcoded sections appended
- Move to AGENTS.md template only

**User note:** "we will have an agent's spec as per claude-code's agent definition specification"

**Decision:** Drop all hardcoded sections. Content moves to user-managed AGENTS.md following CC agent spec.

---

### start_prompt field

**Q:** What should happen to start_prompt in AgentConfig?

**Options presented:**
- Remove from AgentConfig ← **Selected**
- Keep it, embed in system-prompt.txt
- Deprecate — keep field, ignore it

**Decision:** Remove entirely. deny_unknown_fields will cause parse error on existing configs with this field — acceptable (fail-fast).

---

### Default Right agent files

**Q:** Does Phase 24 create SOUL.md, USER.md, AGENTS.md for the default agent?

**Options presented:**
- Yes — create all 3 default files ← **Selected**
- No — leave default agent as-is
- Yes — but only AGENTS.md

**Decision:** Create all 3. AGENTS.md follows CC agent spec (replaces old hardcoded cron + communication content).

---

### File order and separator

**Q1:** File concatenation order?

**User preference:** IDENTITY → SOUL → USER → AGENTS (but "verify how OpenClaw does it — important!")

**Decision:** Preferred IDENTITY → SOUL → USER → AGENTS. Researcher must verify against OpenClaw spec.

---

**Q2:** Separator format?

**User:** "Claude supports embedding files"

**Clarification Q:** @file reference syntax or pre-concatenate?

**Decision:** `@file` reference syntax (e.g., `@IDENTITY.md`). system-prompt.txt lists present files by reference. Researcher must verify that `--system-prompt-file` supports `@include` syntax. If not, fall back to pre-concatenated content with `\n\n---\n\n` separator.

---

## Summary

| Decision | Choice |
|----------|--------|
| PC entries after wrapper removal | Leave broken until Phase 26 |
| Identity file set | All 4 OpenClaw files (IDENTITY + SOUL + USER + AGENTS) |
| File permissions | USER.md writable; others read-only (verify OpenClaw spec) |
| Hardcoded sections | Drop entirely |
| start_prompt field | Remove from AgentConfig |
| Default agent files | Create SOUL.md + USER.md + AGENTS.md |
| system-prompt.txt format | @file reference syntax (verify CC support first) |
| File order | IDENTITY → SOUL → USER → AGENTS (verify OpenClaw) |
