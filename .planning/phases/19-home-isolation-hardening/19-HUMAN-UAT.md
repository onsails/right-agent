# Phase 19: HOME Isolation Hardening - Human UAT

**Date:** 2026-03-27
**Tester:** (fill in)
**Build:** (commit hash of Plan 01 completion — run `git log --oneline -1` in project root)
**Prerequisites:** project built (`cargo build --workspace --release`), process-compose installed, sqlite3 in PATH. Run all commands from the project root. Use `cargo run --release --bin rightclaw --` instead of `rightclaw` if the binary isn't on PATH.

---

## Test 1: Fresh Init

**Purpose:** Verify `rightclaw init` bootstraps a clean home directory with the default agent scaffolded.

**Setup:** None.

**Commands:**
```sh
rm -rf ~/.rightclaw && cargo run --release --bin rightclaw -- init
```

**Expected:**
- `~/.rightclaw/` directory created
- `~/.rightclaw/agents/right/` exists with `IDENTITY.md` and `agent.yaml`

**Pass criteria:**
```sh
ls ~/.rightclaw/agents/right/IDENTITY.md
ls ~/.rightclaw/agents/right/agent.yaml
```
Both commands exit 0.

---

## Test 2: rightclaw up — file generation

**Purpose:** Verify all per-agent files are generated on startup.

**Setup:** Test 1 must be complete.

**Commands:**
```sh
cargo run --release --bin rightclaw -- up --debug
# Wait until agents are running (process-compose TUI shows them up), then Ctrl+C
```

**Expected — for the `right` agent:**
- `~/.rightclaw/agents/right/memory.db` exists
- `~/.rightclaw/agents/right/.claude/settings.json` exists
- `~/.rightclaw/agents/right/.claude.json` exists
- `~/.rightclaw/agents/right/.mcp.json` exists
- Credential symlinks exist (`.anthropic` or `.config`)

**Pass criteria:**
```sh
ls ~/.rightclaw/agents/right/memory.db \
   ~/.rightclaw/agents/right/.claude/settings.json \
   ~/.rightclaw/agents/right/.claude.json \
   ~/.rightclaw/agents/right/.mcp.json
```
All files present (exit 0).

---

## Test 3: .mcp.json content validation

**Purpose:** Verify `RC_AGENT_NAME` is injected and no legacy `"telegram"` marker is present.

**Setup:** Test 2 must be complete.

**Commands:**
```sh
cat ~/.rightclaw/agents/right/.mcp.json | python3 -m json.tool
```

**Expected:**
- `mcpServers.rightmemory.env.RC_AGENT_NAME` equals `"right"`
- No `"telegram": true` key at the root level of the JSON

**Pass criteria:**
```sh
# Should print: "right"
python3 -c "
import json, sys
d = json.load(open('$HOME/.rightclaw/agents/right/.mcp.json'))
name = d['mcpServers']['rightmemory']['env']['RC_AGENT_NAME']
print(name)
assert name == 'right', f'Expected right, got {name}'
assert 'telegram' not in d, 'telegram key present at root — legacy marker not removed'
print('PASS')
"
```

---

## Test 4: Agent WITHOUT telegram — no channels injection

**Purpose:** Verify a non-telegram agent does not get `--channels` or `enabledPlugins`.

**Setup:** Default `right` agent has no `telegram_token` or `telegram_token_file` in `agent.yaml`.

**Check wrapper** (adjust path if rightclaw uses a different temp dir):
```sh
cat /tmp/rightclaw-run/*/right-wrapper.sh 2>/dev/null || \
  cat ~/.rightclaw/run/*/right-wrapper.sh 2>/dev/null
```

**Check settings:**
```sh
cat ~/.rightclaw/agents/right/.claude/settings.json
```

**Pass criteria:**
```sh
# Must return empty (no match)
grep -- '--channels' /tmp/rightclaw-run/*/right-wrapper.sh 2>/dev/null || \
grep -- '--channels' ~/.rightclaw/run/*/right-wrapper.sh 2>/dev/null
# Must return empty (no match)
python3 -c "
import json
s = json.load(open('$HOME/.rightclaw/agents/right/.claude/settings.json'))
assert 'enabledPlugins' not in s, 'enabledPlugins present — Telegram false-positive not fixed'
print('PASS: enabledPlugins absent')
"
```
Both greps return empty; python3 assertion prints PASS.

---

## Test 5: Agent WITH telegram — channels injected correctly

**Purpose:** Verify a telegram-configured agent gets `--channels` and the bot token is stored.

**Setup:** Fresh init with telegram flags (re-uses your real token):

```sh
rm -rf ~/.rightclaw && \
  cargo run --release --bin rightclaw -- init \
    --telegram-token 8643877926:AAElSkP3vO7JJtNmZCauvb3LDiUCj1xlE9A \
    --telegram-user-id 85743491 && \
  cargo run --release --bin rightclaw -- up --debug
# Ctrl+C once agents are running
```

**Expected:**
- Wrapper script contains `--channels plugin:telegram@claude-plugins-official`
- `.claude/channels/telegram/.env` contains `TELEGRAM_BOT_TOKEN=8643877926:AAElSkP3vO7JJtNmZCauvb3LDiUCj1xlE9A`
- Agent CC session shows "Listening for channel messages" WITHOUT "plugin not installed" error
- Agent's `.claude/plugins` is a symlink to `~/.claude/plugins` (shared with host)

**Pass criteria:**
```sh
# Token file exists with correct content
cat ~/.rightclaw/agents/right/.claude/channels/telegram/.env

# plugins is a symlink to host's plugins dir
ls -la ~/.rightclaw/agents/right/.claude/plugins

# CC shows "Listening for channel messages" — observe in process-compose TUI
# (no "plugin not installed" line should appear)
```
`.env` contains the bot token; `plugins` is a symlink; agent connects to Telegram without errors.

---

## Test 6: Memory round-trip with correct provenance

**Purpose:** Verify `stored_by` reflects the agent name, not `"unknown"`.

**Setup:** Start a `rightclaw up` session and wait for the `right` agent to be interactive.

**Steps:**
1. Attach to the `right` agent (or send it a prompt via process-compose TUI)
2. Ask it to store a memory — e.g., prompt: `"Store a memory: test entry for UAT phase 19"`
3. After the agent confirms, run:

```sh
rightclaw memory list right
```

**Expected:**
- At least one entry is listed
- The `stored_by` column shows `right` (not `unknown`)

**Pass criteria:**
`stored_by` value in the output matches the agent name `right`.

---

## Test 7: rightclaw doctor — all checks pass

**Purpose:** Verify the doctor command reports a clean environment.

**Commands:**
```sh
cargo run --release --bin rightclaw -- doctor
```

**Expected:**
- All checks pass: bubblewrap (Linux) / Seatbelt (macOS), socat, sqlite3, git
- No `FAIL` lines in output

**Pass criteria:**
```sh
cargo run --release --bin rightclaw -- doctor; echo "Exit code: $?"
```
Exit code 0; no lines containing `FAIL` in output.

---

## Results Summary

| Test | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | Fresh Init | [ ] Pass / [ ] Fail | |
| 2 | File Generation | [ ] Pass / [ ] Fail | |
| 3 | .mcp.json Content | [ ] Pass / [ ] Fail | |
| 4 | No Telegram | [ ] Pass / [ ] Fail | |
| 5 | With Telegram | [ ] Pass / [ ] Fail | |
| 6 | Memory Round-trip | [ ] Pass / [ ] Fail | |
| 7 | Doctor | [ ] Pass / [ ] Fail | |

**Overall: [ ] All Pass / [ ] Failures — see notes column**
