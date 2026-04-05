# Phase 8: HOME Isolation & Permission Model - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-24
**Phase:** 08-home-isolation-permission-model
**Areas discussed:** ANTHROPIC_API_KEY forwarding, Missing credentials behavior, denyRead path resolution, HOST .claude.json maintenance, Automated testing

---

## Area Selection

| Option | Selected |
|--------|----------|
| ANTHROPIC_API_KEY forwarding | ✓ |
| Missing credentials behavior | ✓ |
| denyRead path resolution | ✓ |
| HOST .claude.json maintenance | ✓ |

**User note:** "we definitely need to support OAuth with HOME override. If OAuth doesn't work, then we ditch HOME override. So we need to manually test first before finishing this brainstorm."

---

## OAuth Validation Gate

| Option | Description | Selected |
|--------|-------------|----------|
| Set HOME=$agent_dir, run claude, verify auth | Baseline smoke test — no symlink | |
| Test with symlink in place | Tests the HOME-03 approach as specced | |
| Both — baseline then symlink | Baseline first to identify failure mode, then symlink to test fix | ✓ |

**User's choice:** Both — baseline then symlink
**Notes:** This is a blocking gate before Phase 8 can be considered complete. Fallback strategy TBD after test results.

---

## OAuth Fallback (if symlink doesn't fix it)

| Option | Description | Selected |
|--------|-------------|----------|
| ANTHROPIC_API_KEY in env | Use API key instead of OAuth | |
| Drop HOME override entirely | Revert to cwd-based isolation | |
| Decide after the test | Cannot pick fallback without knowing what fails | ✓ |

**User's choice:** Decide after the test

---

## ANTHROPIC_API_KEY Forwarding

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, always forward if set | Propagate from host env, no-op if absent | ✓ |
| Yes, and error if not set | Require it for HOME override mode | |
| No, leave to environment | Don't touch it in wrapper | |

**User's choice:** Yes, always forward if set (recommended)

---

## denyRead — Sandbox Model Clarification

User asked: "I don't understand — can we just allow what's essential?"
User referenced CC docs showing `allowRead: ["."]` is a valid sandbox property.

This changed the approach: instead of `denyRead` for specific sensitive paths, use `allowRead` for a proper read allowlist + `denyRead` to block entire host HOME.

---

## Read Scope (allowRead)

| Option | Description | Selected |
|--------|-------------|----------|
| allowRead: ["."] + denyRead host HOME | Tight: agents read only agent dir | ✓ |
| No allowRead, denyRead sensitive paths | Current approach, absolute paths | |
| No read restrictions | Trust write-allowlist + network-allowlist | |

**User's choice:** allowRead: ["."] + denyRead host HOME (recommended)
**Notes:** Planner must use absolute agent path (not ".") to avoid conflict when agent dir is inside the denied host HOME subtree. Verify allowRead specificity vs denyRead parent semantics.

---

## Missing Credentials Behavior

Discussed as part of OAuth validation gate. Decision: warn and skip symlink if `~/.claude/.credentials.json` doesn't exist. Do not fail fast.

---

## HOST .claude.json Maintenance

| Option | Description | Selected |
|--------|-------------|----------|
| Stop writing to host ~/.claude.json | Per-agent file handles all trust | ✓ |
| Keep writing as backup | Belt + suspenders | |
| Depends on OAuth test result | Defer | |

**User's choice:** Stop writing to host ~/.claude.json (recommended)
**Notes:** Also affects init.rs — pre_trust_directory() must be updated to write per-agent .claude.json.

---

## Automated Testing

**User note:** "We need automated tests using `claude -p` mode with structured JSON output (`--output-format json`). All security assumptions must be tested."

Selected test scenarios (all of them):
1. OAuth under bare HOME override (baseline)
2. OAuth with credential symlink
3. Git env vars forwarded
4. denyRead enforcement
5. allowRead enforcement
6. Any other security boundary tests

---

## Claude's Discretion

- Exact field name for bypass-warning suppression in `.claude.json` (verify empirically)
- Whether to remove or keep host `~/.claude.json` writes from `init.rs` after HOME override
- allowRead/denyRead precedence semantics — test empirically

## Deferred Ideas

- Agent-level `env:` section in agent.yaml for arbitrary env var forwarding
- Strict mode requiring ANTHROPIC_API_KEY
- macOS Keychain strategy (post-test)
