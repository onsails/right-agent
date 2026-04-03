# Phase 35: Token Refresh — Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-03
**Phase:** 35-token-refresh
**Areas discussed:** Token endpoint storage, Bot command scope, Proactive refresh buffer, Doctor report format

---

## Token Endpoint Storage

| Option | Description | Selected |
|--------|-------------|----------|
| Extend CredentialToken (full) | Add token_endpoint + client_id + client_secret | |
| Re-run AS discovery | Re-discover token_endpoint on each refresh | ✓ |
| Separate session file | Store in .mcp-sessions.json | |

**User's choice:** Re-run AS discovery for token_endpoint; store only `client_id` + `client_secret` in CredentialToken
**Notes:** User initially chose re-run AS discovery. Follow-up revealed client_id is still needed (refresh POST requires it, and re-running DCR would create new client registrations — incorrect per OAuth spec). Final decision: minimal extension to CredentialToken with `client_id: Option<String>` + `client_secret: Option<String>` only.

---

## Bot Command Scope

| Option | Description | Selected |
|--------|-------------|----------|
| CLI `rightclaw mcp refresh` | On-demand CLI refresh (REFRESH-01) | |
| Auto-refresh in bot only | Bot background task, no CLI command | ✓ |

**User's choice:** Eliminate CLI command. Bot auto-refreshes in background.
**Notes:** User clarified the architecture: bot process handles all token refresh automatically. REFRESH-01 is superseded.

---

## Proactive Refresh Buffer

| Option | Description | Selected |
|--------|-------------|----------|
| Smart scheduler | Startup check + tokio::sleep until expires_at - 10min | ✓ |
| Periodic polling | tokio::interval every 30min | |
| `rightclaw up` refresh (REFRESH-02) | Refresh before launching agents | |

**User's choice:** Smart scheduler; `rightclaw up` proactive refresh (REFRESH-02) eliminated.
**Notes:** User researched Notion token TTL (~1 hour); 10-minute pre-expiry buffer is the chosen refresh window. Startup-time refresh handles the "bot was down when token expired" case.

---

## Doctor Report Format

| Option | Description | Selected |
|--------|-------------|----------|
| One aggregated check | Single "mcp-tokens" DoctorCheck for all agents | ✓ |
| Per-agent checks | One DoctorCheck per agent | |

**User's choice:** Single aggregated check with Warn if any agent/server has issues.

---

## Claude's Discretion

- Retry strategy for failed refresh (3 retries × 5 min chosen autonomously)
- One scheduler task per token vs per agent
- Telegram notification on refresh failure (log-only vs brief message to user)

## Deferred Ideas

- `/mcp refresh` Telegram bot command — superseded by auto-refresh, not needed
