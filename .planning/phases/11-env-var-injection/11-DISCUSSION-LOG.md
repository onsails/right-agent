# Phase 11: Env Var Injection - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-03-25
**Phase:** 11-env-var-injection
**Areas discussed:** Value expansion, Identity var priority, Generated template

---

## Value Expansion

| Option | Description | Selected |
|--------|-------------|----------|
| Strict literals | Single-quote everything. What you write is what the agent gets. No ${VAR} expansion. | ✓ |
| Host expansion | Double-quote values, ${HOST_VAR} resolves from host env at launch. More flexible, riskier. | |

**User's choice:** Strict literals
**Notes:** No surprises, no injection risk. Users who need host values use file references (telegram_token_file pattern).

---

## Identity Var Priority

| Option | Description | Selected |
|--------|-------------|----------|
| env: wins | Inject env: AFTER identity captures. Per-agent git identity works. Explicit wins. | ✓ |
| Identity always wins | Inject env: BEFORE identity captures. Host identity always takes precedence. | |

**User's choice:** env: wins
**Notes:** User initially asked why identity vars are captured at all — explained HOME override context. After understanding, confirmed env: should win to enable per-agent git identity use case.

---

## Generated Template

| Option | Description | Selected |
|--------|-------------|----------|
| Commented example | `# env:\n#   MY_VAR: "value"  # plaintext only` in generated agent.yaml | ✓ |
| No section | Don't show env: in generated file at all | |

**User's choice:** Commented example
**Notes:** Clear and discoverable without adding clutter.

---

## Claude's Discretion

- Rust map type for `env:` field (IndexMap vs HashMap)
- Whether env: block uses Jinja for-loop or pre-rendered export lines
- startup_prompt quoting fix scope

## Deferred Ideas

- ${VAR} host expansion — explicitly rejected
