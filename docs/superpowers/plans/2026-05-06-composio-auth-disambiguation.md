# Composio auth disambiguation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the agent from suggesting `/mcp auth <server>` as a fix-all whenever a tool response contains the word "auth"; let upstream tools' own diagnostic instructions (e.g. Composio's `status_message`) flow through unaltered.

**Architecture:** Pure prompt edit — single file, three textual changes inside one section of `OPERATING_INSTRUCTIONS.md`. The file is embedded into the binary via `include_str!` at compile time, so the change reaches running agents on the next bot restart.

**Tech Stack:** Rust workspace (edition 2024). The prompt template is consumed by `crates/right-agent/src/codegen/agent_def.rs` via `include_str!("../../templates/right/prompt/OPERATING_INSTRUCTIONS.md")`. No code changes; rebuild + restart picks it up.

**TDD note:** No unit test applies — this is a prompt heuristic. The "failing test" is the production observation already captured in the spec (agent told user `/mcp auth composio` for a Google Docs connection in the `aibots` chat at 15:12 UTC on 2026-05-06). The "passing test" is a manual verification after restart (Task 3).

**Spec:** `docs/superpowers/specs/2026-05-06-composio-auth-disambiguation-design.md`.

---

## Files

- Modify: `crates/right-agent/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (lines 184–189, plus an inserted note after line 189)

That is the only file touched in this plan. The duplicate copy at the repo root (`templates/right/prompt/OPERATING_INSTRUCTIONS.md`) is **intentionally not** modified — it is dead, not referenced by `include_str!`. Hygiene cleanup tracked separately (see spec § "Out of scope").

---

## Task 1: Edit OPERATING_INSTRUCTIONS.md

**Files:**
- Modify: `crates/right-agent/templates/right/prompt/OPERATING_INSTRUCTIONS.md` (lines 184–189 and insertion after 189)

- [ ] **Step 1: Replace the broad auth-pattern row in the diagnosis table**

In `crates/right-agent/templates/right/prompt/OPERATING_INSTRUCTIONS.md`, replace this exact line (currently line 186):

```
| "unauthorized", "forbidden", "auth", 401, 403 | Authentication/permission problem | Tell the user to run `/mcp auth <server>` |
```

with:

```
| HTTP 401/403 from MCP transport, OR error string `Authentication required for '<server>'. Use /mcp auth <server>` (raised by Right Agent's proxy when the OAuth token is missing/expired) | MCP-transport-level auth: Right Agent ↔ MCP server | Tell the user to run `/mcp auth <server>` |
```

The remaining three rows (Validation error / connection refused / not found) stay unchanged.

- [ ] **Step 2: Insert a new row at the end of the table**

Immediately after the "not found, unknown tool" row (currently line 189), add this row as the new last row of the table:

```
| Tool response payload itself contains a status/instruction field (e.g. `status_message`, `error.message`, `instructions`) telling you what to do next | Upstream tool already diagnosed the issue and prescribed the fix | Follow the upstream instruction verbatim. Do NOT translate it into `/mcp auth` advice. |
```

- [ ] **Step 3: Insert a clarifying paragraph after the table**

Between the table and the existing `**Critical:** "missing fields" means YOUR request is malformed …` paragraph, insert this paragraph (with one blank line on either side):

```
**Trust upstream diagnostics.** When a tool's own response payload tells you what action to take ("call X to set up connection", "visit URL Y to authorize", etc.), follow it as-is. `/mcp auth` is a Right Agent CLI command for re-authorizing the MCP transport — it is not a fix-all for any authentication-shaped error inside tool responses.
```

- [ ] **Step 4: Rebuild the workspace so the binary embeds the new prompt**

Run: `cd /Users/molt/dev/rightclaw && cargo build --workspace`
Expected: compiles cleanly. Two reasons to build the whole workspace, not just `right-agent`:
  1. The `OPERATING_INSTRUCTIONS.md` is `include_str!`'d at compile time inside `right-agent`, but the running binary is `right` (in crate `right`) and `right-bot` (in crate `bot`). Both must be rebuilt so the new prompt is actually shipped.
  2. Markdown content is opaque to the compiler — a green build only proves the file still exists at the expected path; it does NOT validate the prompt's behavioral effect (that's Task 3).

- [ ] **Step 5: Commit**

```bash
cd /Users/molt/dev/rightclaw
git add crates/right-agent/templates/right/prompt/OPERATING_INSTRUCTIONS.md
git commit -m "feat(prompt): narrow MCP auth-error rule, trust upstream diagnostics

The MCP Error Diagnosis table matched broadly on \"auth\" and reflexively
suggested /mcp auth <server>. Composio's own response payload already
told the agent how to fix per-app connection state (call
COMPOSIO_MANAGE_CONNECTIONS); our prompt overrode it. Narrow the auth
rule to MCP-transport signals only (HTTP 401/403 or our own
ProxyError::NeedsAuth string) and add a row + note instructing the
agent to follow upstream payload diagnostics verbatim.

Spec: docs/superpowers/specs/2026-05-06-composio-auth-disambiguation-design.md"
```

---

## Task 2: Restart him-bot to pick up the new prompt

**Files:** none (operational)

- [ ] **Step 1: Restart the agent**

Run: `/Users/molt/dev/rightclaw/target/debug/right restart him`
(If the binary path differs in your environment, use whatever `right` resolves to. The restart goes through `process-compose` and re-launches `him-bot` with the rebuilt binary so the new embedded prompt takes effect.)

Expected: command exits 0; `him-bot` reappears in `right status` shortly afterwards.

- [ ] **Step 2: Confirm him-bot is back up**

Run: `curl -s -H "X-PC-Token-Key: $(jq -r .pc_api_token ~/.right/run/state.json)" "http://localhost:$(jq -r .pc_port ~/.right/run/state.json)/processes" | jq '.data[] | select(.name=="him-bot") | {name, status, pid}'`

Expected output: `{"name":"him-bot","status":"Running","pid":<some int>}`.

---

## Task 3: Manual behavioral verification (user-driven)

**Files:** none (observation only)

This task requires the operator (Andrey) to interact with the running bot in the `aibots` Telegram chat (chat_id `-4996137249`). It cannot be automated from this plan.

- [ ] **Step 1: Reproduce the original scenario**

In the `aibots` chat, send the same kind of request that triggered the bug — anything that requires the agent to read or modify a Google Doc through Composio. Concrete example (matches the original 15:12 UTC interaction): ask the agent to read or update the same Google Doc URL it tried to read at 15:12 UTC (find the URL in `~/.right/logs/him.log.2026-05-06` near the `mcp__right__composio__COMPOSIO_MULTI_EXECUTE_TOOL` calls in turns 8 and 10 of session at that time).

- [ ] **Step 2: Observe the agent's response in chat**

Expected (any one of these counts as success):
- Agent calls `COMPOSIO_MANAGE_CONNECTIONS` (the tool Composio's `status_message` instructs it to use) and continues.
- Agent relays Composio's `status_message` verbatim to the user (e.g., "No Active connection for toolkit=googledocs. Call COMPOSIO_MANAGE_CONNECTIONS …" or a paraphrase that preserves the actionable instruction).
- Agent directs the user to authorize Google Docs inside Composio's own surface (e.g., `https://app.composio.dev`).

Failure mode (the regression we are fixing):
- Agent says "run `/mcp auth composio`" or any close paraphrase of that.

- [ ] **Step 3: Cross-check the log**

Run: `rg "/mcp auth composio" ~/.right/logs/him.log.$(date -u +%Y-%m-%d) | rg -v "telegram::dispatch"`

Expected: zero new matches in `📝` (assistant text) or `🔧` lines after the Task 2 restart timestamp. The `telegram::dispatch` filter excludes any user-typed `/mcp auth composio` commands that would also appear in the log — we only care about the agent's own outputs.

If the failure mode appears, the prompt heuristic was insufficient. Escalate to spec § "Future work" → Option B (Composio-specific `with_instructions()` block).

---

## Self-Review Notes

- **Spec coverage:** All three changes from the spec (narrow rule / new row / clarifying note) map to Task 1 steps 1, 2, 3. Verification scenario from spec maps to Task 3.
- **No placeholders:** Each edit step contains the literal text. Verification step gives concrete commands and concrete expected output.
- **Type consistency:** N/A — no types involved (prompt-only change).
- **Risk handling:** Spec's two risks (false negative + ambiguity) are not test-coverable in advance; Task 3 step 3 + the escalation note handle the "did the heuristic work?" question post-hoc.
