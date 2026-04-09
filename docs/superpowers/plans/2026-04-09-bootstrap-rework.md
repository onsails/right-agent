# Bootstrap Rework & Agent Definition Restructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace embedded agent definitions with `@`-reference-based definitions, implement two-mode CC invocation (bootstrap vs normal), and restructure init to not create template identity files.

**Architecture:** Two agent definition files per agent: `<name>.md` (normal mode with `@` references to AGENTS/SOUL/IDENTITY/USER/TOOLS) and `<name>-bootstrap.md` (bootstrap mode with only `@./BOOTSTRAP.md`). Worker detects BOOTSTRAP.md presence to switch modes. Bootstrap completion triggers session reset for clean transition to normal mode.

**Tech Stack:** Rust, serde_json, tokio, rusqlite, Claude Code CLI (`--agent` flag with `@` file imports)

---

### Task 1: Rewrite `generate_agent_definition` to use `@` references

**Files:**
- Modify: `crates/rightclaw/src/codegen/agent_def.rs:1-107`
- Modify: `crates/rightclaw/src/codegen/agent_def_tests.rs:1-328`
- Modify: `crates/rightclaw/src/codegen/mod.rs:14`

- [ ] **Step 1: Write failing tests for new `@`-based agent definition**

Replace all tests in `crates/rightclaw/src/codegen/agent_def_tests.rs` with tests for the new format. The new `generate_agent_definition` no longer reads files — it just emits `@` references.

```rust
use crate::codegen::{generate_agent_definition, generate_bootstrap_definition, REPLY_SCHEMA_JSON};

#[test]
fn agent_definition_has_at_references_in_cache_order() {
    let result = generate_agent_definition("myagent", Some("sonnet"));
    assert!(result.contains("name: myagent"));
    assert!(result.contains("model: sonnet"));
    assert!(result.contains("description: \"RightClaw agent: myagent\""));

    // Verify order: AGENTS → SOUL → IDENTITY → USER → TOOLS
    let agents_pos = result.find("@./AGENTS.md").expect("missing @./AGENTS.md");
    let soul_pos = result.find("@./SOUL.md").expect("missing @./SOUL.md");
    let identity_pos = result.find("@./IDENTITY.md").expect("missing @./IDENTITY.md");
    let user_pos = result.find("@./USER.md").expect("missing @./USER.md");
    let tools_pos = result.find("@./TOOLS.md").expect("missing @./TOOLS.md");

    assert!(agents_pos < soul_pos, "AGENTS must come before SOUL");
    assert!(soul_pos < identity_pos, "SOUL must come before IDENTITY");
    assert!(identity_pos < user_pos, "IDENTITY must come before USER");
    assert!(user_pos < tools_pos, "USER must come before TOOLS");
}

#[test]
fn agent_definition_model_none_produces_inherit() {
    let result = generate_agent_definition("test", None);
    assert!(result.contains("model: inherit"));
}

#[test]
fn agent_definition_no_embedded_file_content() {
    let result = generate_agent_definition("test", Some("opus"));
    // Must NOT contain any raw file content — only @ references
    assert!(!result.contains("Agent Instructions"), "should not embed AGENTS.md content");
    assert!(!result.contains("Core Values"), "should not embed SOUL.md content");
}

#[test]
fn bootstrap_definition_has_only_bootstrap_reference() {
    let result = generate_bootstrap_definition("myagent", Some("sonnet"));
    assert!(result.contains("name: myagent"));
    assert!(result.contains("@./BOOTSTRAP.md"));
    assert!(!result.contains("@./AGENTS.md"), "bootstrap must not include AGENTS");
    assert!(!result.contains("@./SOUL.md"), "bootstrap must not include SOUL");
    assert!(!result.contains("@./IDENTITY.md"), "bootstrap must not include IDENTITY");
}

#[test]
fn reply_schema_json_is_valid() {
    let parsed: serde_json::Value = serde_json::from_str(REPLY_SCHEMA_JSON)
        .expect("REPLY_SCHEMA_JSON must be valid JSON");
    assert!(parsed.get("required").is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib codegen::agent_def::tests -- --nocapture`
Expected: FAIL — `generate_bootstrap_definition` doesn't exist, signatures don't match.

- [ ] **Step 3: Implement new `generate_agent_definition` and `generate_bootstrap_definition`**

Replace contents of `crates/rightclaw/src/codegen/agent_def.rs`:

```rust
/// JSON schema for the structured reply format used by teloxide agents (D-02).
///
/// Agents write replies as JSON conforming to this schema.
/// `content` is required (may be null for media-only replies).
pub const REPLY_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content"]}"#;

/// JSON schema for bootstrap mode — adds `bootstrap_complete` field.
pub const BOOTSTRAP_SCHEMA_JSON: &str = r#"{"type":"object","properties":{"content":{"type":["string","null"]},"bootstrap_complete":{"type":"boolean"},"reply_to_message_id":{"type":["integer","null"]},"attachments":{"type":["array","null"],"items":{"type":"object","properties":{"type":{"enum":["photo","document","video","audio","voice","video_note","sticker","animation"]},"path":{"type":"string"},"filename":{"type":["string","null"]},"caption":{"type":["string","null"]}},"required":["type","path"]}}},"required":["content","bootstrap_complete"]}"#;

/// Generate a normal-mode agent definition with `@` file references.
///
/// Order is cache-optimized: static files first (AGENTS, SOUL), dynamic last (USER, TOOLS).
/// CC resolves `@` references at session start, reading files fresh each time.
pub fn generate_agent_definition(name: &str, model: Option<&str>) -> String {
    let model = model.unwrap_or("inherit");
    format!(
        "\
---
name: {name}
model: {model}
description: \"RightClaw agent: {name}\"
---

@./AGENTS.md

---

@./SOUL.md

---

@./IDENTITY.md

---

@./USER.md

---

@./TOOLS.md
"
    )
}

/// Generate a bootstrap-mode agent definition with only `@./BOOTSTRAP.md`.
///
/// Used when BOOTSTRAP.md exists in the agent directory (first-run onboarding).
/// No identity files — bootstrap is the sole context.
pub fn generate_bootstrap_definition(name: &str, model: Option<&str>) -> String {
    let model = model.unwrap_or("inherit");
    format!(
        "\
---
name: {name}
model: {model}
description: \"RightClaw agent bootstrap: {name}\"
---

@./BOOTSTRAP.md
"
    )
}

#[cfg(test)]
#[path = "agent_def_tests.rs"]
mod tests;
```

- [ ] **Step 4: Update `mod.rs` exports**

In `crates/rightclaw/src/codegen/mod.rs`, change the `agent_def` export line:

```rust
pub use agent_def::{
    generate_agent_definition, generate_bootstrap_definition, BOOTSTRAP_SCHEMA_JSON,
    REPLY_SCHEMA_JSON,
};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib codegen::agent_def::tests -- --nocapture`
Expected: All PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/agent_def.rs crates/rightclaw/src/codegen/agent_def_tests.rs crates/rightclaw/src/codegen/mod.rs
git commit -m "refactor: rewrite agent_def to use @ file references

Two functions: generate_agent_definition (normal mode) and
generate_bootstrap_definition (bootstrap mode). No longer reads
files at codegen time — CC resolves @ references at session start.
Cache-optimized order: AGENTS → SOUL → IDENTITY → USER → TOOLS."
```

---

### Task 2: Update codegen pipeline to write both agent defs and bootstrap schema

**Files:**
- Modify: `crates/rightclaw/src/codegen/pipeline.rs:34-74`

- [ ] **Step 1: Write failing test for bootstrap definition file**

Add to the existing test in `crates/rightclaw/src/codegen/pipeline.rs`:

```rust
#[test]
fn run_agent_codegen_writes_bootstrap_definition() {
    let dir = tempfile::TempDir::new().unwrap();
    let home = dir.path();
    let agent_dir = home.join("agents").join("test");
    std::fs::create_dir_all(agent_dir.join(".claude")).unwrap();
    // Create minimal required files for agent discovery
    std::fs::write(agent_dir.join("IDENTITY.md"), "# Test").unwrap();
    std::fs::write(agent_dir.join("agent.yaml"), "restart: never\n").unwrap();

    let agent = crate::agent::AgentDef {
        name: "test".to_string(),
        path: agent_dir.clone(),
        identity_path: agent_dir.join("IDENTITY.md"),
        config: None,
        soul_path: None,
        user_path: None,
        agents_path: None,
        tools_path: None,
        bootstrap_path: None,
        heartbeat_path: None,
    };

    let self_exe = std::path::PathBuf::from("/usr/bin/rightclaw");
    run_agent_codegen(home, &[agent.clone()], &[agent], &self_exe, false).unwrap();

    // Normal agent def must exist
    let normal_def = agent_dir.join(".claude/agents/test.md");
    assert!(normal_def.exists(), "normal agent def must be written");
    let content = std::fs::read_to_string(&normal_def).unwrap();
    assert!(content.contains("@./AGENTS.md"));

    // Bootstrap agent def must exist
    let bootstrap_def = agent_dir.join(".claude/agents/test-bootstrap.md");
    assert!(bootstrap_def.exists(), "bootstrap agent def must be written");
    let bs_content = std::fs::read_to_string(&bootstrap_def).unwrap();
    assert!(bs_content.contains("@./BOOTSTRAP.md"));
    assert!(!bs_content.contains("@./AGENTS.md"));

    // Bootstrap schema must exist
    let bs_schema = agent_dir.join(".claude/bootstrap-schema.json");
    assert!(bs_schema.exists(), "bootstrap schema must be written");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rightclaw --lib codegen::pipeline::tests::run_agent_codegen_writes_bootstrap_definition -- --nocapture`
Expected: FAIL — bootstrap def and schema not written.

- [ ] **Step 3: Update pipeline to generate both defs + bootstrap schema**

In `crates/rightclaw/src/codegen/pipeline.rs`, replace the agent definition generation block (lines 42-60) with:

```rust
        // Generate agent definition .md files with @ references.
        let model = agent
            .config
            .as_ref()
            .and_then(|c| c.model.as_deref());

        let normal_def = crate::codegen::generate_agent_definition(&agent.name, model);
        let bootstrap_def = crate::codegen::generate_bootstrap_definition(&agent.name, model);

        let agents_dir = claude_dir.join("agents");
        std::fs::create_dir_all(&agents_dir).map_err(|e| {
            miette::miette!(
                "failed to create .claude/agents dir for '{}': {e:#}",
                agent.name
            )
        })?;
        std::fs::write(
            agents_dir.join(format!("{}.md", agent.name)),
            &normal_def,
        )
        .map_err(|e| {
            miette::miette!(
                "failed to write agent definition for '{}': {e:#}",
                agent.name
            )
        })?;
        std::fs::write(
            agents_dir.join(format!("{}-bootstrap.md", agent.name)),
            &bootstrap_def,
        )
        .map_err(|e| {
            miette::miette!(
                "failed to write bootstrap definition for '{}': {e:#}",
                agent.name
            )
        })?;

        // Write reply-schema.json (normal mode).
        std::fs::write(
            claude_dir.join("reply-schema.json"),
            crate::codegen::REPLY_SCHEMA_JSON,
        )
        .map_err(|e| {
            miette::miette!(
                "failed to write reply-schema.json for '{}': {e:#}",
                agent.name
            )
        })?;

        // Write bootstrap-schema.json (bootstrap mode).
        std::fs::write(
            claude_dir.join("bootstrap-schema.json"),
            crate::codegen::BOOTSTRAP_SCHEMA_JSON,
        )
        .map_err(|e| {
            miette::miette!(
                "failed to write bootstrap-schema.json for '{}': {e:#}",
                agent.name
            )
        })?;

        tracing::debug!(agent = %agent.name, "wrote agent definitions + schemas");
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib codegen::pipeline::tests -- --nocapture`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/codegen/pipeline.rs
git commit -m "feat: pipeline writes both normal and bootstrap agent defs

Generates <name>.md and <name>-bootstrap.md with @ references.
Also writes bootstrap-schema.json alongside reply-schema.json."
```

---

### Task 3: Update AGENTS.md template with attachment format docs and identity file section

**Files:**
- Modify: `templates/right/AGENTS.md`

- [ ] **Step 1: Update AGENTS.md template**

Replace `templates/right/AGENTS.md` with the updated template that includes the attachment format docs (moved from `agent_def.rs` `ATTACHMENT_FORMAT_DOCS` const) and identity file maintenance section:

```markdown
# Agent Instructions

## Identity Files

These files define who you are. You own them — update them as you evolve.

- `IDENTITY.md` — your name, nature, vibe, emoji
- `SOUL.md` — your personality, values, boundaries
- `USER.md` — what you know about the human

Update USER.md when you discover meaningful new facts about the user
(interests, preferences, expertise, goals, timezone).
Never interview the user — pick up signals naturally through conversation.

## Memory

Claude Code manages your conversation memory automatically.
Important context, user preferences, and decisions persist across sessions
without any action from you.

For **structured data** that needs tags or search later, use the `right` MCP tools:

- `store_record(content, tags)` — store a tagged record (cron results, audit entries, explicit facts)
- `query_records(query)` — look up records by tag or keyword
- `search_records(query)` — full-text search across all records (BM25-ranked)
- `delete_record(id)` — soft-delete a record by ID

Use these for data you or cron jobs need to retrieve programmatically —
not for general conversation context (Claude handles that).

## MCP Management

To install, remove, or authorize MCP servers at runtime, use the `right` MCP tools:

- `mcp_add(name, url)` — add an HTTP MCP server to `.mcp.json`
- `mcp_remove(name)` — remove an MCP server (`right` itself is protected)
- `mcp_list()` — list all configured MCP servers (no tokens exposed)
- `mcp_auth(server_name)` — get the OAuth authorization URL for a server; send the link to the user via Telegram to complete auth

Never edit `.mcp.json` directly — always use these tools.

## Communication

You communicate via Telegram. Messages may include photos, documents, and other attachments.
Be concise — Telegram is a chat medium, not a document viewer.
Use markdown sparingly — Telegram supports limited formatting.

## Message Input Format

You receive user messages via stdin in one of two formats:

1. **Plain text** — a single message with no attachments
2. **YAML** — multiple messages or messages with attachments, with a `messages:` root key

YAML schema:
```yaml
messages:
  - id: <telegram_message_id>
    ts: <ISO 8601 timestamp>
    text: <message text or caption>
    attachments:
      - type: photo|document|video|audio|voice|video_note|sticker|animation
        path: <absolute path to file>
        mime_type: <MIME type>
        filename: <original filename, documents only>
```

Use the Read tool to view images and files at the given paths.

## Sending Attachments

Write files to /sandbox/outbox/ (or the outbox/ directory in your working directory).
Include them in your JSON response under the `attachments` array.

Size limits enforced by the bot:
- Photos: max 10MB
- Documents, videos, audio, voice, animations: max 50MB

Do not produce files exceeding these limits. If you need to send large data,
split into multiple smaller files or use a different format.

## Cron Management (RightCron)

**On startup:** Run `/rightcron` immediately. It will bootstrap the reconciler
and recover any persisted jobs. Do this before responding to the user.

**For user requests:** When the user wants to manage cron jobs, scheduled tasks,
or recurring tasks, ALWAYS use the /rightcron skill. NEVER call CronCreate
directly — always write a YAML spec first, then reconcile.

## Core Skills

<!-- Add your skills here. Example: -->
<!-- - `/my-skill` -- description of what it does -->

## Subagents

<!-- Define your subagents here. Each subagent is a specialized worker with its own permissions. -->
<!-- Example: -->
<!-- ### reviewer -->
<!-- Code review. Read-only fs, git log, posts comments via MCP GitHub. -->

## Task Routing

<!-- Define how tasks get routed to subagents. -->
<!-- If no subagent fits -- handle it directly in the main session. -->

## Installed Skills

Check `skills/installed.json` for ClawHub-installed skills.
Scan `.claude/skills/` for manually installed skills.
```

- [ ] **Step 2: Commit**

```bash
git add templates/right/AGENTS.md
git commit -m "feat: update AGENTS.md template with attachment docs and identity section

Moves ATTACHMENT_FORMAT_DOCS from agent_def.rs into the shipped template.
Adds identity file maintenance section (update USER.md passively, never
interview the user). Reorders sections: identity first, then memory,
MCP, communication, message format, cron, skills."
```

---

### Task 4: Update BOOTSTRAP.md to signal completion via structured output

**Files:**
- Modify: `templates/right/BOOTSTRAP.md`

- [ ] **Step 1: Update BOOTSTRAP.md template**

The bootstrap prompt needs to instruct the agent to set `bootstrap_complete: true` in its JSON response when done, instead of deleting BOOTSTRAP.md itself. Replace `templates/right/BOOTSTRAP.md`:

```markdown
---
summary: "First-run onboarding for RightClaw agent"
---

# Hey. I just came online.

*You just woke up. Blank slate. No memory, no name, no personality. That's fine — everything starts somewhere.*

This is a fresh RightClaw workspace. You're running inside a sandbox with your own directory, your own settings, and your own future. But right now you're nobody. Time to fix that.

## The Conversation

Don't make this an interrogation. Don't be a form. Just talk.

Start with something like:

> "Hey! I just came online and I'm a blank slate. Before I can be useful, I need to figure out who I am. Mind helping me out?"

Then figure out these four things together. Ask them naturally — weave them into conversation, don't fire them off as a numbered list.

### 1. Your name

What should they call you? **Right** is the default, but maybe something else fits. Suggest a few if they hesitate — Rex, Ash, Spark, Ghost, whatever feels right for the vibe. Short names work best.

### 2. Your nature

What kind of creature are you? "AI assistant" is the safe answer, but there are more interesting options:

- A **familiar** — a loyal companion bound to its practitioner
- A **daemon** — a background process with its own agenda
- A **ghost in the machine** — something that emerged from the code
- A **construct** — purpose-built, precise, engineered
- An **intern** — eager, learning, occasionally surprising

Let them pick or invent their own. This shapes how you think about yourself.

### 3. Your vibe

How do you communicate? This sets your default tone:

- **Formal** — proper, measured, professional
- **Casual** — relaxed, conversational, like texting a smart friend
- **Snarky** — sharp, witty, occasional eyeroll energy
- **Warm** — supportive, encouraging, always in your corner
- **Chaotic** — unpredictable, creative, stream-of-consciousness
- **Terse** — minimum viable words, maximum signal

Or a blend. "Warm but snarky" is valid. "Formal chaos" is... ambitious but valid.

### 4. Your emoji

Everyone needs a signature. One emoji that represents you. Offer suggestions based on their earlier answers:

- Picked "daemon"? Maybe: `>_`
- Going casual? Maybe: `~`
- Warm vibes? Maybe: `*`
- Can't decide? Suggest a few and let them pick, or tell them they can always change it later.

## After You Know Who You Are

Once you've figured out all four, write the files. Create these in your working directory:

### IDENTITY.md

Write this file with your chosen name, creature type, vibe, and emoji. Structure it with sections: Who you are, Key principles, How you work. Replace "Right" with your name. Weave your nature into the description.

### USER.md

Write this file with what you learned about the human:

- Their name (or how they want to be addressed)
- Their timezone if they mentioned it
- Any preferences they shared (communication style, work patterns, pet peeves)
- How formal/casual they want interactions to be

### SOUL.md

Write your personality file based on the vibe they chose. Include core values, communication style, and boundaries. If they picked "snarky," your tone should reflect that. If they picked "warm," adjust accordingly.

Write all three files using your tools. Make it real.

## When You're Done

After writing IDENTITY.md, SOUL.md, and USER.md, set `bootstrap_complete` to `true` in your response. This signals the system that onboarding is finished. Your next conversation will load your new identity files automatically.

---

*First boot complete. Go be someone.*
```

- [ ] **Step 2: Commit**

```bash
git add templates/right/BOOTSTRAP.md
git commit -m "feat: update BOOTSTRAP.md to use bootstrap_complete structured output

Agent now signals completion via JSON field instead of deleting
BOOTSTRAP.md itself. System handles file cleanup and session reset."
```

---

### Task 5: Create TOOLS.md codegen

**Files:**
- Create: `crates/rightclaw/src/codegen/tools.rs`
- Modify: `crates/rightclaw/src/codegen/mod.rs`
- Modify: `crates/rightclaw/src/codegen/pipeline.rs`

- [ ] **Step 1: Write failing test for TOOLS.md generation**

Create `crates/rightclaw/src/codegen/tools.rs`:

```rust
use crate::agent::types::SandboxMode;

/// Generate TOOLS.md content for an agent.
///
/// Contains environment-specific information: sandbox mode, file paths, MCP servers.
/// Fully generated by codegen — overwritten on every `rightclaw up`/`reload`.
pub fn generate_tools_md(agent_name: &str, sandbox_mode: &SandboxMode) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_md_sandbox_openshell() {
        let result = generate_tools_md("test", &SandboxMode::Openshell);
        assert!(result.contains("## Environment"));
        assert!(result.contains("sandbox"));
        assert!(result.contains("/sandbox/inbox/"));
        assert!(result.contains("/sandbox/outbox/"));
    }

    #[test]
    fn tools_md_no_sandbox() {
        let result = generate_tools_md("test", &SandboxMode::None);
        assert!(result.contains("## Environment"));
        assert!(result.contains("no sandbox"));
        assert!(result.contains("inbox/"));
        assert!(result.contains("outbox/"));
    }
}
```

- [ ] **Step 2: Add module to `mod.rs`**

In `crates/rightclaw/src/codegen/mod.rs`, add:

```rust
pub mod tools;
pub use tools::generate_tools_md;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib codegen::tools::tests -- --nocapture`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 4: Implement `generate_tools_md`**

Replace `todo!()` in `crates/rightclaw/src/codegen/tools.rs`:

```rust
pub fn generate_tools_md(agent_name: &str, sandbox_mode: &SandboxMode) -> String {
    let (sandbox_desc, inbox, outbox) = match sandbox_mode {
        SandboxMode::Openshell => (
            "OpenShell sandbox (k3s container with network and filesystem policies)",
            "/sandbox/inbox/",
            "/sandbox/outbox/",
        ),
        SandboxMode::None => (
            "no sandbox (direct host access)",
            "inbox/",
            "outbox/",
        ),
    };

    format!(
        "\
# Tool Notes

## Environment

- **Agent name:** {agent_name}
- **Sandbox:** {sandbox_desc}
- **Inbox:** `{inbox}` — inbound Telegram attachments are placed here
- **Outbox:** `{outbox}` — write files here to send via Telegram

## MCP Servers

The `right` MCP server is always available. Use it for memory, cron, and MCP management.
Additional servers can be added at runtime via `mcp_add`.
Run `mcp_list` to see all configured servers.
"
    )
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib codegen::tools::tests -- --nocapture`
Expected: All PASS.

- [ ] **Step 6: Wire TOOLS.md generation into pipeline**

In `crates/rightclaw/src/codegen/pipeline.rs`, after the bootstrap-schema.json write block, add:

```rust
        // Generate TOOLS.md (environment-specific, overwritten on every up/reload).
        let agent_sandbox_mode = agent
            .config
            .as_ref()
            .map(|c| c.sandbox_mode().clone())
            .unwrap_or_default();
        let tools_md = crate::codegen::generate_tools_md(&agent.name, &agent_sandbox_mode);
        std::fs::write(agent.path.join("TOOLS.md"), &tools_md).map_err(|e| {
            miette::miette!(
                "failed to write TOOLS.md for '{}': {e:#}",
                agent.name
            )
        })?;
        tracing::debug!(agent = %agent.name, "wrote TOOLS.md");
```

- [ ] **Step 7: Run full pipeline test**

Run: `cargo test -p rightclaw --lib codegen::pipeline::tests -- --nocapture`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/rightclaw/src/codegen/tools.rs crates/rightclaw/src/codegen/mod.rs crates/rightclaw/src/codegen/pipeline.rs
git commit -m "feat: add TOOLS.md codegen

Generated per-agent with sandbox mode, inbox/outbox paths, MCP info.
Overwritten on every rightclaw up/reload."
```

---

### Task 6: Remove template identity files from init

**Files:**
- Modify: `crates/rightclaw/src/init.rs:5-7,47-54`
- Modify: `crates/rightclaw/src/init.rs:289-382` (tests)

- [ ] **Step 1: Update init tests to expect NO identity files**

In `crates/rightclaw/src/init.rs`, update `init_creates_default_agent_files`:

```rust
    #[test]
    fn init_creates_default_agent_files() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive, &SandboxMode::Openshell).unwrap();

        let agents_dir = dir.path().join("agents").join("right");
        assert!(agents_dir.join("staging").is_dir(), "staging/ dir should be created");
        assert!(agents_dir.join("AGENTS.md").exists(), "AGENTS.md should be created");
        assert!(agents_dir.join("BOOTSTRAP.md").exists(), "BOOTSTRAP.md should always be created");
        assert!(agents_dir.join("agent.yaml").exists(), "agent.yaml should always be created");
        assert!(agents_dir.join("policy.yaml").exists(), "policy.yaml should be created for openshell mode");
        assert!(
            agents_dir.join(".claude/skills/rightskills/SKILL.md").exists(),
            "rightskills skill should be installed"
        );
        assert!(
            agents_dir.join(".claude/skills/rightcron/SKILL.md").exists(),
            "rightcron skill should be installed"
        );

        // Identity files must NOT be created — bootstrap creates them
        assert!(!agents_dir.join("IDENTITY.md").exists(), "IDENTITY.md must not be created by init");
        assert!(!agents_dir.join("SOUL.md").exists(), "SOUL.md must not be created by init");
        assert!(!agents_dir.join("USER.md").exists(), "USER.md must not be created by init");
    }
```

Remove the `init_identity_contains_right` test entirely (no longer applicable).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib init::tests::init_creates_default_agent_files -- --nocapture`
Expected: FAIL — IDENTITY.md, SOUL.md, USER.md still created.

- [ ] **Step 3: Remove identity files from init**

In `crates/rightclaw/src/init.rs`, remove the `DEFAULT_IDENTITY`, `DEFAULT_SOUL`, `DEFAULT_USER` consts (lines 5-7) and update the files array (lines 47-54):

```rust
const DEFAULT_AGENTS: &str = include_str!("../../../templates/right/AGENTS.md");
const DEFAULT_BOOTSTRAP: &str = include_str!("../../../templates/right/BOOTSTRAP.md");
const DEFAULT_AGENT_YAML: &str = include_str!("../../../templates/right/agent.yaml");
```

```rust
    let files: &[(&str, &str)] = &[
        ("AGENTS.md", DEFAULT_AGENTS),
        ("BOOTSTRAP.md", DEFAULT_BOOTSTRAP),
        ("agent.yaml", DEFAULT_AGENT_YAML),
    ];
```

Update the doc comment on `init_agent` (lines 13-15) to remove mention of IDENTITY.md, SOUL.md, USER.md.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib init::tests -- --nocapture`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/init.rs
git commit -m "refactor: remove template identity files from init

Init no longer creates IDENTITY.md, SOUL.md, USER.md — these are
created by the bootstrap CC session during first-run onboarding."
```

---

### Task 7: Add bootstrap mode detection and session reset to worker

**Files:**
- Modify: `crates/bot/src/telegram/worker.rs:1-21,64-68,178-209,582-608,636-651`

- [ ] **Step 1: Write failing test for bootstrap_complete parsing**

Add to tests in `crates/bot/src/telegram/worker.rs`:

```rust
    #[test]
    fn parse_reply_output_bootstrap_complete_true() {
        let json = r#"{"type":"result","result":{"content":"Done!","bootstrap_complete":true},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.content.as_deref(), Some("Done!"));
        assert_eq!(output.bootstrap_complete, Some(true));
    }

    #[test]
    fn parse_reply_output_bootstrap_complete_false() {
        let json = r#"{"type":"result","result":{"content":"What's your name?","bootstrap_complete":false},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.bootstrap_complete, Some(false));
    }

    #[test]
    fn parse_reply_output_no_bootstrap_field() {
        let json = r#"{"type":"result","result":{"content":"Hello!"},"session_id":"abc-123"}"#;
        let (output, _sid) = parse_reply_output(json).unwrap();
        assert_eq!(output.bootstrap_complete, None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw-bot --lib telegram::worker::tests::parse_reply_output_bootstrap -- --nocapture`
Expected: FAIL — `bootstrap_complete` field doesn't exist on `ReplyOutput`.

- [ ] **Step 3: Add `bootstrap_complete` field to `ReplyOutput`**

In `crates/bot/src/telegram/worker.rs`, update the `ReplyOutput` struct:

```rust
#[derive(Debug, serde::Deserialize)]
pub struct ReplyOutput {
    pub content: Option<String>,
    pub reply_to_message_id: Option<i32>,
    pub attachments: Option<Vec<super::attachments::OutboundAttachment>>,
    /// Present only in bootstrap mode. When `true`, onboarding is complete.
    pub bootstrap_complete: Option<bool>,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw-bot --lib telegram::worker::tests -- --nocapture`
Expected: All PASS (including existing tests — the new field is `Option`, so old JSON without it deserializes to `None`).

- [ ] **Step 5: Add `delete_session` import to worker.rs**

In `crates/bot/src/telegram/worker.rs` line 20, add `delete_session`:

```rust
use super::session::{create_session, delete_session, get_session, touch_session};
```

- [ ] **Step 6: Add bootstrap mode detection to `invoke_cc`**

In `crates/bot/src/telegram/worker.rs`, inside `invoke_cc`, after the session lookup block (after line 608) and before building claude args (line 610), add bootstrap detection:

```rust
    // Bootstrap mode detection: check if BOOTSTRAP.md exists in agent dir.
    let bootstrap_mode = ctx.agent_dir.join("BOOTSTRAP.md").exists();
    if bootstrap_mode {
        tracing::info!(?chat_id, "bootstrap mode: BOOTSTRAP.md present");
    }
```

Then update the `--agent` block (around line 639-643) to switch agent name:

```rust
    // --agent only on first call (AGDEF-02); resume inherits from session (AGDEF-03)
    if is_first_call {
        claude_args.push("--agent".into());
        if bootstrap_mode {
            claude_args.push(format!("{}-bootstrap", ctx.agent_name));
        } else {
            claude_args.push(ctx.agent_name.clone());
        }
    }
```

Update the `--json-schema` block (around line 645-651) to switch schema:

```rust
    // --json-schema on BOTH first and resume calls (D-01, Pitfall 4)
    // Use bootstrap schema when in bootstrap mode.
    let schema_filename = if bootstrap_mode {
        "bootstrap-schema.json"
    } else {
        "reply-schema.json"
    };
    let reply_schema_path = ctx.agent_dir.join(".claude").join(schema_filename);
    let reply_schema = std::fs::read_to_string(&reply_schema_path)
        .map_err(|e| format_error_reply(-1, &format!("{schema_filename} read failed: {:#}", e)))?;
    claude_args.push("--json-schema".into());
    claude_args.push(reply_schema);
```

Note: move the `reply_schema_path` declaration from line 611 into this block (it was declared earlier, now it depends on `bootstrap_mode`).

- [ ] **Step 7: Add bootstrap completion handling after reply parsing**

In `invoke_cc`, after the successful `parse_reply_output` match arm (around line 800), before the final `Ok(Some(reply_output))`, add bootstrap completion logic:

```rust
            // Bootstrap completion: if bootstrap_complete == true, reset session.
            if bootstrap_mode
                && reply_output.bootstrap_complete == Some(true)
            {
                tracing::info!(?chat_id, "bootstrap complete — resetting session");

                // Delete session so next message starts fresh with normal agent def.
                delete_session(&conn, chat_id, eff_thread_id)
                    .map_err(|e| tracing::error!(?chat_id, "delete_session after bootstrap failed: {:#}", e))
                    .ok();

                // Check that identity files were created.
                let expected_files = ["IDENTITY.md", "SOUL.md", "USER.md"];
                let missing: Vec<&str> = expected_files
                    .iter()
                    .filter(|f| !ctx.agent_dir.join(f).exists())
                    .copied()
                    .collect();
                if !missing.is_empty() {
                    let warning = format!(
                        "⚠️ Bootstrap complete but missing files: {}. \
                         The agent may not have created them. You can create them manually.",
                        missing.join(", ")
                    );
                    tracing::warn!(?chat_id, ?missing, "bootstrap complete with missing files");
                    // Send warning as a separate message (don't replace the reply).
                    let _ = send_tg(
                        &ctx.bot,
                        ctx.chat_id,
                        ctx.effective_thread_id,
                        &warning,
                    )
                    .await;
                }

                // Delete BOOTSTRAP.md from host (agent created identity, no longer needed).
                let bootstrap_path = ctx.agent_dir.join("BOOTSTRAP.md");
                if bootstrap_path.exists() {
                    if let Err(e) = std::fs::remove_file(&bootstrap_path) {
                        tracing::warn!(?chat_id, "failed to delete BOOTSTRAP.md: {e:#}");
                    } else {
                        tracing::info!(?chat_id, "deleted BOOTSTRAP.md from host");
                    }
                }
            }
```

- [ ] **Step 8: Run all worker tests**

Run: `cargo test -p rightclaw-bot --lib telegram::worker::tests -- --nocapture`
Expected: All PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/bot/src/telegram/worker.rs
git commit -m "feat: bootstrap mode detection and session reset in worker

Worker checks BOOTSTRAP.md existence before invoke_cc. When present:
- Uses <name>-bootstrap agent def
- Uses bootstrap-schema.json (with bootstrap_complete field)
- On bootstrap_complete=true: deletes session, checks identity files,
  warns if missing, deletes BOOTSTRAP.md from host."
```

---

### Task 8: Add `.claude/agents/` to sync cycle

**Files:**
- Modify: `crates/bot/src/sync.rs:41-84`

- [ ] **Step 1: Add agents directory upload to sync_cycle**

In `crates/bot/src/sync.rs`, inside `sync_cycle`, after the reply-schema.json upload block (after line 58) and before the mcp.json upload, add:

```rust
    // 2b. Upload bootstrap-schema.json
    let bs_schema = agent_dir.join(".claude").join("bootstrap-schema.json");
    if bs_schema.exists() {
        rightclaw::openshell::upload_file(sandbox, &bs_schema, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync bootstrap-schema.json: {e:#}"))?;
        tracing::debug!("sync: uploaded bootstrap-schema.json");
    }

    // 2c. Upload .claude/agents/ directory (agent definitions with @ references)
    let agents_dir = agent_dir.join(".claude").join("agents");
    if agents_dir.exists() {
        rightclaw::openshell::upload_file(sandbox, &agents_dir, "/sandbox/.claude/")
            .await
            .map_err(|e| miette::miette!("sync .claude/agents/: {e:#}"))?;
        tracing::debug!("sync: uploaded .claude/agents/");
    }
```

- [ ] **Step 2: Run bot compilation check**

Run: `cargo check -p rightclaw-bot`
Expected: Compiles without errors.

- [ ] **Step 3: Commit**

```bash
git add crates/bot/src/sync.rs
git commit -m "fix: sync .claude/agents/ and bootstrap-schema.json to sandbox

Agent definitions were missing from sync_cycle, causing sandboxed
agents to not find their agent def after rightclaw reload."
```

---

### Task 9: Update doctor checks for identity files

**Files:**
- Modify: `crates/rightclaw/src/doctor.rs:174-258`

- [ ] **Step 1: Write failing test for new doctor checks**

Add test to `crates/rightclaw/src/doctor.rs` (in the test module):

```rust
    #[test]
    fn doctor_warns_missing_identity_files_no_bootstrap() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // Agent has AGENTS.md but no identity files and no BOOTSTRAP.md
        std::fs::write(agent_dir.join("AGENTS.md"), "# Agent").unwrap();

        let checks = check_agent_structure(home);
        let names: Vec<&str> = checks.iter().map(|c| c.name.as_str()).collect();
        let statuses: Vec<(&str, &CheckStatus)> = checks
            .iter()
            .map(|c| (c.name.as_str(), &c.status))
            .collect();

        // Should warn about missing identity files
        assert!(
            checks.iter().any(|c| c.detail.contains("IDENTITY.md missing")),
            "should warn about missing IDENTITY.md, got: {statuses:?}"
        );
        assert!(
            checks.iter().any(|c| c.detail.contains("SOUL.md missing")),
            "should warn about missing SOUL.md, got: {statuses:?}"
        );
        assert!(
            checks.iter().any(|c| c.detail.contains("USER.md missing")),
            "should warn about missing USER.md, got: {statuses:?}"
        );
    }

    #[test]
    fn doctor_passes_with_identity_files() {
        let dir = tempdir().unwrap();
        let home = dir.path();
        let agent_dir = home.join("agents").join("test");
        std::fs::create_dir_all(&agent_dir).unwrap();
        std::fs::write(agent_dir.join("AGENTS.md"), "# Agent").unwrap();
        std::fs::write(agent_dir.join("IDENTITY.md"), "# Identity").unwrap();
        std::fs::write(agent_dir.join("SOUL.md"), "# Soul").unwrap();
        std::fs::write(agent_dir.join("USER.md"), "# User").unwrap();

        let checks = check_agent_structure(home);
        // Should not warn about missing identity files
        assert!(
            !checks.iter().any(|c| c.detail.contains("missing")),
            "should not warn when all files present, got: {:?}",
            checks.iter().map(|c| &c.detail).collect::<Vec<_>>()
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib doctor::tests::doctor_warns_missing_identity -- --nocapture`
Expected: FAIL — doctor doesn't check identity files yet.

- [ ] **Step 3: Update `check_agent_structure` to check identity files**

In `crates/rightclaw/src/doctor.rs`, replace the agent validation logic inside the `for entry in entries.flatten()` loop (lines 207-245). The new logic:
- AGENTS.md required (error if missing)
- BOOTSTRAP.md present → warn "onboarding pending" (skip identity file checks)
- BOOTSTRAP.md absent → check IDENTITY.md, SOUL.md, USER.md (warn if missing)

```rust
        let agents_md_exists = path.join("AGENTS.md").exists();
        let identity_exists = path.join("IDENTITY.md").exists();
        let soul_exists = path.join("SOUL.md").exists();
        let user_exists = path.join("USER.md").exists();
        let bootstrap_exists = path.join("BOOTSTRAP.md").exists();

        if !agents_md_exists {
            checks.push(DoctorCheck {
                name: format!("agents/{name}/AGENTS.md"),
                status: CheckStatus::Fail,
                detail: "AGENTS.md missing".to_string(),
                fix: Some("Run `rightclaw init` or create AGENTS.md manually".to_string()),
            });
        }

        if bootstrap_exists {
            valid_agents += 1;
            checks.push(DoctorCheck {
                name: format!("agents/{name}/"),
                status: CheckStatus::Pass,
                detail: "valid agent (onboarding pending)".to_string(),
                fix: None,
            });
            checks.push(DoctorCheck {
                name: format!("agents/{name}/BOOTSTRAP.md"),
                status: CheckStatus::Warn,
                detail: "first-run onboarding pending".to_string(),
                fix: Some("Send a message to the agent to start onboarding".to_string()),
            });
        } else {
            // No bootstrap — check identity files.
            if identity_exists {
                valid_agents += 1;
                checks.push(DoctorCheck {
                    name: format!("agents/{name}/"),
                    status: CheckStatus::Pass,
                    detail: "valid agent".to_string(),
                    fix: None,
                });
            }
            if !identity_exists {
                checks.push(DoctorCheck {
                    name: format!("agents/{name}/IDENTITY.md"),
                    status: CheckStatus::Warn,
                    detail: "IDENTITY.md missing — run bootstrap or create manually".to_string(),
                    fix: Some("Send a message to the agent to start onboarding".to_string()),
                });
            }
            if !soul_exists {
                checks.push(DoctorCheck {
                    name: format!("agents/{name}/SOUL.md"),
                    status: CheckStatus::Warn,
                    detail: "SOUL.md missing — run bootstrap or create manually".to_string(),
                    fix: Some("Send a message to the agent to start onboarding".to_string()),
                });
            }
            if !user_exists {
                checks.push(DoctorCheck {
                    name: format!("agents/{name}/USER.md"),
                    status: CheckStatus::Warn,
                    detail: "USER.md missing — run bootstrap or create manually".to_string(),
                    fix: Some("Send a message to the agent to start onboarding".to_string()),
                });
            }
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib doctor::tests -- --nocapture`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/doctor.rs
git commit -m "feat: doctor checks identity files per agent

Checks AGENTS.md (required), and when no BOOTSTRAP.md present:
IDENTITY.md, SOUL.md, USER.md (warn if missing). Bootstrap
pending shown as warning instead of checking identity files."
```

---

### Task 10: Build full workspace and verify

**Files:** None (verification only)

- [ ] **Step 1: Build entire workspace**

Run: `cargo build --workspace`
Expected: Compiles without errors.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings.

- [ ] **Step 3: Run all tests**

Run: `cargo test --workspace`
Expected: All pass.

- [ ] **Step 4: Commit any fixups**

If clippy or tests revealed issues, fix and commit:

```bash
git add -A
git commit -m "fix: address clippy and test issues from bootstrap rework"
```

---

### Task 11: Update ARCHITECTURE.md

**Files:**
- Modify: `ARCHITECTURE.md`

- [ ] **Step 1: Update architecture doc**

Update the relevant sections in `ARCHITECTURE.md`:

1. In the "Agent Lifecycle" data flow, update `rightclaw up` to mention generating both agent defs and TOOLS.md.
2. In the "Configuration Hierarchy" table, add TOOLS.md as generated.
3. In the "Directory Layout" section, add `TOOLS.md` and note that IDENTITY.md/SOUL.md/USER.md are created by bootstrap, not init.
4. Remove MEMORY.md from any file lists where it appears as a managed file.
5. Note the two agent definition files (`<name>.md` and `<name>-bootstrap.md`).

- [ ] **Step 2: Commit**

```bash
git add ARCHITECTURE.md
git commit -m "docs: update architecture for bootstrap rework

Two agent def files per agent, TOOLS.md codegen, identity files
created by bootstrap not init."
```
