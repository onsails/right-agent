# Phase 31: E2E Verification - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-02
**Phase:** 31-e2e-verification
**Areas discussed:** Verification format, Sandbox signal parsing, Live vs simulated, Scope of E2E

---

## Verification Format

| Option | Description | Selected |
|--------|-------------|----------|
| Shell script | Bash script in tests/e2e/ — programmatic pass/fail, re-runnable after CC upgrades | ✓ |
| Rust integration test | assert_cmd-based test — structured but requires real CC binary + agent setup | |
| Markdown checklist | Manual checklist — low-tech but not automatable | |

**User's choice:** Shell script

**Follow-up: Script depth**

| Option | Description | Selected |
|--------|-------------|----------|
| Dependencies only | Check rg/socat/bwrap in PATH, validate settings.json. No CC invocation. | |
| Dependencies + CC smoke test | Same + launch claude -p and parse output. Requires CC binary + API key. | ✓ |
| Full flow test | Dependencies + CC smoke + Telegram send + cron fire. | |

**User's choice:** Dependencies + CC smoke test, with structured output (--json-schema) and haiku model for cost efficiency.

**Follow-up: Agent setup**

| Option | Description | Selected |
|--------|-------------|----------|
| Existing agent | Script takes agent name, uses real settings.json. Requires rightclaw up. | ✓ |
| Ephemeral temp agent | Creates minimal temp dir. Self-contained but synthetic. | |

**User's choice:** Existing agent

---

## Sandbox Signal Parsing

| Option | Description | Selected |
|--------|-------------|----------|
| Structured output | Use --json-schema for structured CC output. Parse JSON + exit code. | ✓ |
| Stderr pattern match | Grep CC stderr for warning strings. Brittle across CC versions. | |
| Settings.json + exit code | Verify failIfUnavailable:true, check exit code only. Simple but indirect. | |

**User's choice:** Structured output

**Follow-up: Logging**

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, stderr to log | Redirect CC stderr to tests/e2e/last-run.log. Print on failure. | ✓ |
| No, just exit code | Keep it simple — exit code + structured output only. | |

**User's choice:** Yes, stderr to log

---

## Live vs Simulated

| Option | Description | Selected |
|--------|-------------|----------|
| Single CC smoke test | One claude -p invocation from agent dir. If CC+sandbox works here, it works for bot/cron too. | ✓ |
| Separate bot + cron tests | Full live: Telegram message + cron trigger. Requires bot token + active crons. | |
| Simulate both paths | Two CC invocations with worker.rs/cron.rs env vars. Without Telegram/cron infra. | |

**User's choice:** Single CC smoke test — bot and cron use same binary, same settings, same env vars.

---

## Scope of E2E

**Follow-up: Doctor gate**

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, doctor first | Run rightclaw doctor pre-flight, parse for Fail/Warn. Skip CC smoke if doctor fails. | ✓ |
| No, independent | Script checks deps independently. Doctor is separate. | |

**User's choice:** Yes, doctor first

**Follow-up: Prerequisites**

| Option | Description | Selected |
|--------|-------------|----------|
| Require pre-existing up | Script checks settings.json exists. Errors if not. Read-only verification. | ✓ |
| Run rightclaw up inside script | Script calls rightclaw up to generate fresh config. Self-contained but mutates state. | |

**User's choice:** Require pre-existing up

---

## Claude's Discretion

- Exact structured output JSON schema for the smoke test prompt
- Doctor output parsing strategy (regex vs jq vs string match)
- Script argument handling (flags, defaults, help text)
- Color/formatting of pass/fail output

## Deferred Ideas

- "Document CC gotcha — Telegram messages dropped while agent is streaming" — reviewed, not folded (docs task, out of scope)
