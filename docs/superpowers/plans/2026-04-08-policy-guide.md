# Network Policy Choice & Policy Guide Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a restrictive/permissive network policy choice during `rightclaw init`, update policy codegen to branch on this, and document policy customization in SECURITY.md.

**Architecture:** New `NetworkPolicy` enum on `AgentConfig` drives policy codegen branching. Init flow prompts user. SECURITY.md gets a "Configuring Policies" section. README.md gets an anchor link.

**Tech Stack:** Rust (serde, clap, miette), Markdown

**Spec:** `docs/superpowers/specs/2026-04-08-policy-guide-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/rightclaw/src/agent/types.rs` | Modify | Add `NetworkPolicy` enum + field on `AgentConfig` |
| `crates/rightclaw/src/codegen/policy.rs` | Modify | Branch policy generation on `NetworkPolicy` |
| `crates/rightclaw/src/init.rs` | Modify | Accept `NetworkPolicy`, write to agent.yaml, add prompt fn |
| `crates/rightclaw-cli/src/main.rs` | Modify | Add `--network-policy` CLI flag, pass to init + policy codegen |
| `crates/rightclaw-cli/tests/cli_integration.rs` | Modify | Update `generate_policy` call site |
| `templates/right/agent.yaml` | Modify | Add commented `network_policy` example |
| `docs/SECURITY.md` | Modify | Add "Configuring Policies" section |
| `README.md` | Modify | Add anchor link to policy guide |

---

### Task 1: Add `NetworkPolicy` enum to `AgentConfig`

**Files:**
- Modify: `crates/rightclaw/src/agent/types.rs:1-45`

- [ ] **Step 1: Write failing test for `NetworkPolicy` deserialization**

Add to the `#[cfg(test)] mod tests` block at the bottom of `types.rs`:

```rust
    #[test]
    fn network_policy_defaults_to_permissive() {
        let yaml = "{}";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Permissive);
    }

    #[test]
    fn network_policy_deserializes_restrictive() {
        let yaml = "network_policy: restrictive";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Restrictive);
    }

    #[test]
    fn network_policy_deserializes_permissive() {
        let yaml = "network_policy: permissive";
        let config: AgentConfig = serde_saphyr::from_str(yaml).unwrap();
        assert_eq!(config.network_policy, NetworkPolicy::Permissive);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib agent::types::tests::network_policy`
Expected: FAIL — `NetworkPolicy` type does not exist yet

- [ ] **Step 3: Implement `NetworkPolicy` enum and add field**

Add the enum before `SandboxOverrides` (around line 6):

```rust
/// Network access policy for sandbox.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// Only allow Anthropic/Claude domains.
    Restrictive,
    /// Allow all outbound HTTPS (default for backwards compat).
    #[default]
    Permissive,
}
```

Add field to `AgentConfig` struct (after `backoff_seconds`):

```rust
    /// Network access policy: restrictive (Anthropic only) or permissive (all HTTPS).
    #[serde(default)]
    pub network_policy: NetworkPolicy,
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p rightclaw --lib agent::types::tests`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw/src/agent/types.rs
git commit -m "feat: add NetworkPolicy enum to AgentConfig"
```

---

### Task 2: Branch policy codegen on `NetworkPolicy`

**Files:**
- Modify: `crates/rightclaw/src/codegen/policy.rs:1-119`

- [ ] **Step 1: Write failing tests for restrictive policy**

Add to the `#[cfg(test)] mod tests` block in `policy.rs`:

```rust
    #[test]
    fn restrictive_policy_allows_only_anthropic_domains() {
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive);
        assert!(policy.contains(r#"host: "*.anthropic.com""#));
        assert!(policy.contains(r#"host: "anthropic.com""#));
        assert!(policy.contains(r#"host: "*.claude.com""#));
        assert!(policy.contains(r#"host: "claude.com""#));
        assert!(policy.contains(r#"host: "*.claude.ai""#));
        assert!(!policy.contains(r#"host: "**.*""#), "restrictive must not contain wildcard");
    }

    #[test]
    fn permissive_policy_allows_all_https() {
        let policy = generate_policy(8100, &NetworkPolicy::Permissive);
        assert!(policy.contains(r#"host: "**.*""#));
        assert!(!policy.contains(r#"host: "*.anthropic.com""#), "permissive uses wildcard, not explicit domains");
    }

    #[test]
    fn restrictive_policy_is_valid_yaml() {
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive);
        let parsed: serde_json::Value = serde_saphyr::from_str(&policy)
            .expect("restrictive policy must be valid YAML");
        let obj = parsed.as_object().expect("policy root must be a mapping");
        assert!(obj.contains_key("network_policies"));
    }

    #[test]
    fn restrictive_policy_has_no_bare_star_wildcards() {
        let policy = generate_policy(8100, &NetworkPolicy::Restrictive);
        for line in policy.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("host:") {
                let host_val = trimmed.trim_start_matches("host:").trim().trim_matches('"');
                assert_ne!(
                    host_val, "*",
                    "bare '*' wildcard rejected by OpenShell — use '*.domain.com'"
                );
            }
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib codegen::policy::tests`
Expected: FAIL — `generate_policy` signature doesn't accept `NetworkPolicy`

- [ ] **Step 3: Update `generate_policy` to accept `NetworkPolicy` and branch**

Add import at top of `policy.rs`:

```rust
use crate::agent::types::NetworkPolicy;
```

Change the function signature and body:

```rust
pub fn generate_policy(right_mcp_port: u16, network_policy: &NetworkPolicy) -> String {
    let network_section = match network_policy {
        NetworkPolicy::Permissive => {
            r#"  outbound:
    endpoints:
      - host: "**.*"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: "**""#
                .to_string()
        }
        NetworkPolicy::Restrictive => {
            r#"  anthropic:
    endpoints:
      - host: "*.anthropic.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "anthropic.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "*.claude.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "claude.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
      - host: "*.claude.ai"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: "**""#
                .to_string()
        }
    };

    format!(
        r#"version: 1

filesystem_policy:
  include_workdir: true
  read_only:
    - /usr
    - /lib
    - /lib64
    - /etc
    - /proc
    - /dev/urandom
    - /dev/null
  read_write:
    - /tmp
    - /sandbox

landlock:
  compatibility: best_effort

process:
  run_as_user: sandbox
  run_as_group: sandbox

network_policies:
{network_section}

  right:
    endpoints:
      - host: "host.docker.internal"
        port: {right_mcp_port}
        allowed_ips:
          - "172.16.0.0/12"
        protocol: rest
        access: full
    binaries:
      - path: "**"
"#
    )
}
```

- [ ] **Step 4: Update existing tests to pass `NetworkPolicy::Permissive`**

Update all existing test calls from `generate_policy(8100)` to `generate_policy(8100, &NetworkPolicy::Permissive)` and `generate_policy(9000)` to `generate_policy(9000, &NetworkPolicy::Permissive)`. There are 5 existing tests to update:
- `generates_policy_with_right_mcp_port`
- `allows_all_outbound_https`
- `right_mcp_port_configurable`
- `no_bare_star_host_wildcards`
- `policy_is_valid_yaml_with_required_sections`

- [ ] **Step 5: Run all policy tests**

Run: `cargo test -p rightclaw --lib codegen::policy::tests`
Expected: ALL PASS (existing + new)

- [ ] **Step 6: Commit**

```bash
git add crates/rightclaw/src/codegen/policy.rs
git commit -m "feat: branch policy codegen on NetworkPolicy (restrictive/permissive)"
```

---

### Task 3: Update all `generate_policy` call sites

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs:790-801`
- Modify: `crates/rightclaw-cli/tests/cli_integration.rs:326`

- [ ] **Step 1: Update `cmd_up` in `main.rs`**

At line 797, the call currently is:
```rust
let policy_yaml = rightclaw::codegen::policy::generate_policy(rightclaw::runtime::MCP_HTTP_PORT);
```

Change to read the agent's network_policy from config:
```rust
            let network_policy = agent
                .config
                .as_ref()
                .map(|c| &c.network_policy)
                .unwrap_or(&rightclaw::agent::types::NetworkPolicy::Permissive);
            let policy_yaml = rightclaw::codegen::policy::generate_policy(
                rightclaw::runtime::MCP_HTTP_PORT,
                network_policy,
            );
```

- [ ] **Step 2: Update integration test call site**

In `crates/rightclaw-cli/tests/cli_integration.rs` line 326, change:
```rust
        rightclaw::codegen::policy::generate_policy(rightclaw::runtime::MCP_HTTP_PORT);
```
to:
```rust
        rightclaw::codegen::policy::generate_policy(
            rightclaw::runtime::MCP_HTTP_PORT,
            &rightclaw::agent::types::NetworkPolicy::Permissive,
        );
```

- [ ] **Step 3: Build workspace to verify all call sites compile**

Run: `cargo build --workspace`
Expected: SUCCESS (no compile errors)

- [ ] **Step 4: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs crates/rightclaw-cli/tests/cli_integration.rs
git commit -m "fix: update generate_policy call sites with NetworkPolicy param"
```

---

### Task 4: Add network policy prompt to init flow

**Files:**
- Modify: `crates/rightclaw/src/init.rs:23-139`
- Modify: `crates/rightclaw-cli/src/main.rs:118-138,400-458`

- [ ] **Step 1: Write failing test for `NetworkPolicy` in agent.yaml output**

Add to `init.rs` test module:

```rust
    #[test]
    fn init_writes_network_policy_restrictive_to_agent_yaml() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Restrictive).unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("network_policy: restrictive"),
            "agent.yaml must contain network_policy: restrictive, got:\n{yaml}"
        );
    }

    #[test]
    fn init_writes_network_policy_permissive_to_agent_yaml() {
        let dir = tempdir().unwrap();
        init_rightclaw_home(dir.path(), None, &[], &NetworkPolicy::Permissive).unwrap();

        let yaml = std::fs::read_to_string(dir.path().join("agents/right/agent.yaml")).unwrap();
        assert!(
            yaml.contains("network_policy: permissive"),
            "agent.yaml must contain network_policy: permissive, got:\n{yaml}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rightclaw --lib init::tests::init_writes_network_policy`
Expected: FAIL — `init_rightclaw_home` doesn't accept `NetworkPolicy`

- [ ] **Step 3: Update `init_rightclaw_home` signature and write network_policy to agent.yaml**

Add import at top of `init.rs`:
```rust
use crate::agent::types::NetworkPolicy;
```

Change signature:
```rust
pub fn init_rightclaw_home(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    network_policy: &NetworkPolicy,
) -> miette::Result<()> {
```

After writing template files (after the `for` loop, around line 59), append `network_policy` to agent.yaml:

```rust
    // Write network policy to agent.yaml.
    {
        let agent_yaml_path = agents_dir.join("agent.yaml");
        let mut yaml = std::fs::read_to_string(&agent_yaml_path)
            .map_err(|e| miette::miette!("Failed to read agent.yaml: {}", e))?;
        let policy_str = match network_policy {
            NetworkPolicy::Restrictive => "restrictive",
            NetworkPolicy::Permissive => "permissive",
        };
        yaml.push_str(&format!("\nnetwork_policy: {policy_str}\n"));
        std::fs::write(&agent_yaml_path, yaml)
            .map_err(|e| miette::miette!("Failed to update agent.yaml: {}", e))?;
    }
```

- [ ] **Step 4: Update all existing `init_rightclaw_home` calls in tests**

All existing test calls in `init.rs` need the new parameter. Update every `init_rightclaw_home(dir.path(), ...)` call to add `&NetworkPolicy::Permissive` as the last argument. There are ~10 test functions to update.

- [ ] **Step 5: Add `prompt_network_policy` function**

Add to `init.rs` after `prompt_telegram_token`:

```rust
/// Prompt the user for network policy choice interactively.
///
/// Returns the chosen `NetworkPolicy`. Defaults to `Restrictive` on empty input.
pub fn prompt_network_policy() -> miette::Result<NetworkPolicy> {
    use std::io::{self, Write};
    println!("Network policy for sandbox:");
    println!("  1. Restrictive — Anthropic/Claude domains only (recommended)");
    println!("  2. Permissive — all HTTPS domains allowed (needed for external MCP servers)");
    print!("Choose [1/2] (default: 1): ");
    io::stdout().flush().map_err(|e| miette::miette!("stdout flush failed: {e}"))?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| miette::miette!("failed to read input: {e}"))?;
    match input.trim() {
        "" | "1" => Ok(NetworkPolicy::Restrictive),
        "2" => Ok(NetworkPolicy::Permissive),
        other => Err(miette::miette!("Invalid choice: '{other}'. Expected 1 or 2.")),
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test -p rightclaw --lib init::tests`
Expected: ALL PASS

- [ ] **Step 7: Commit**

```bash
git add crates/rightclaw/src/init.rs
git commit -m "feat: add network_policy param to init + interactive prompt"
```

---

### Task 5: Wire CLI flag for network policy

**Files:**
- Modify: `crates/rightclaw-cli/src/main.rs:118-138,308-311,400-458`

- [ ] **Step 1: Add `--network-policy` flag to `Init` command**

In the `Init` variant of `Commands` enum (around line 121), add:

```rust
        /// Network policy: restrictive (Anthropic/Claude only) or permissive (all HTTPS)
        #[arg(long, value_parser = parse_network_policy)]
        network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
```

Add the parser function near the top of `main.rs` (before `Commands`):

```rust
fn parse_network_policy(s: &str) -> Result<rightclaw::agent::types::NetworkPolicy, String> {
    match s {
        "restrictive" => Ok(rightclaw::agent::types::NetworkPolicy::Restrictive),
        "permissive" => Ok(rightclaw::agent::types::NetworkPolicy::Permissive),
        other => Err(format!("invalid network policy: '{other}'. Expected 'restrictive' or 'permissive'.")),
    }
}
```

- [ ] **Step 2: Update `Commands::Init` match arm to pass `network_policy`**

At line 309, update the match arm:

```rust
        Commands::Init { telegram_token, telegram_allowed_chat_ids, tunnel_name, tunnel_hostname, yes, network_policy } => {
            cmd_init(&home, telegram_token.as_deref(), &telegram_allowed_chat_ids, &tunnel_name, tunnel_hostname.as_deref(), yes, network_policy)
        }
```

- [ ] **Step 3: Update `cmd_init` to handle network policy**

Update `cmd_init` signature:

```rust
fn cmd_init(
    home: &Path,
    telegram_token: Option<&str>,
    telegram_allowed_chat_ids: &[i64],
    tunnel_name: &str,
    tunnel_hostname: Option<&str>,
    yes: bool,
    network_policy: Option<rightclaw::agent::types::NetworkPolicy>,
) -> miette::Result<()> {
```

Add network policy resolution after the `chat_ids` block (around line 428):

```rust
    // Network policy: CLI flag > interactive prompt > restrictive (default).
    let network_policy = match network_policy {
        Some(p) => p,
        None if !interactive => rightclaw::agent::types::NetworkPolicy::Restrictive,
        None => rightclaw::init::prompt_network_policy()?,
    };
```

Update the `init_rightclaw_home` call:

```rust
    rightclaw::init::init_rightclaw_home(home, token.as_deref(), &chat_ids, &network_policy)?;
```

Add output after init:

```rust
    let policy_label = match &network_policy {
        rightclaw::agent::types::NetworkPolicy::Restrictive => "restrictive (Anthropic/Claude only)",
        rightclaw::agent::types::NetworkPolicy::Permissive => "permissive (all HTTPS)",
    };
    println!("Network policy: {policy_label}");
```

- [ ] **Step 4: Build workspace**

Run: `cargo build --workspace`
Expected: SUCCESS

- [ ] **Step 5: Commit**

```bash
git add crates/rightclaw-cli/src/main.rs
git commit -m "feat: add --network-policy CLI flag to rightclaw init"
```

---

### Task 6: Update template agent.yaml

**Files:**
- Modify: `templates/right/agent.yaml`

- [ ] **Step 1: Add commented network_policy example**

The current template is:
```yaml
model: sonnet                 # Model to use (sonnet, opus, haiku)
restart: on_failure          # Restart policy: on_failure, always, never
max_restarts: 5              # Maximum restart attempts
backoff_seconds: 10          # Delay between restarts
```

Add after `backoff_seconds`:

```yaml
# network_policy: restrictive  # restrictive = Anthropic/Claude only, permissive = all HTTPS
```

Note: the actual `network_policy` value is appended by `init_rightclaw_home`, so the template just shows a comment explaining the options.

- [ ] **Step 2: Commit**

```bash
git add templates/right/agent.yaml
git commit -m "docs: add network_policy comment to agent.yaml template"
```

---

### Task 7: Add "Configuring Policies" section to SECURITY.md

**Files:**
- Modify: `docs/SECURITY.md:38-52`

- [ ] **Step 1: Add the new section after "Declarative Policies" (after line 38)**

Insert before the "Prompt Injection Guard" section:

```markdown
## Configuring Policies

**Default behavior:** Out of the box with `network_policy: permissive`, agents can reach any HTTPS endpoint. All traffic still goes through OpenShell's proxy with TLS termination for inspection — but no domain restrictions apply.

With `network_policy: restrictive`, only Anthropic and Claude domains are allowed:
- `*.anthropic.com`, `anthropic.com`
- `*.claude.com`, `claude.com`
- `*.claude.ai`

**Setting during init:**

`rightclaw init` prompts for this choice interactively. You can also pass it directly:

```sh
rightclaw init --network-policy restrictive
```

**Changing after init:**

Edit `network_policy` in your agent's `agent.yaml`:

```yaml
network_policy: restrictive   # or: permissive
```

Then run `rightclaw up` to regenerate and apply the policy.

**Custom domain allowlists:**

For fine-grained control beyond restrictive/permissive, edit the generated policy directly:

```
~/.rightclaw/run/policies/<agent>.yaml
```

Add endpoint entries under `network_policies` following OpenShell's format. For example, to allow an MCP server in restrictive mode:

```yaml
  notion_mcp:
    endpoints:
      - host: "mcp.notion.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: "**"
```

> **Note:** `rightclaw up` regenerates policy files on every launch. Manual edits will be overwritten. Edit the policy after `rightclaw up` completes for each run.
```

- [ ] **Step 2: Commit**

```bash
git add docs/SECURITY.md
git commit -m "docs: add Configuring Policies section to SECURITY.md"
```

---

### Task 8: Update README.md Security section with anchor link

**Files:**
- Modify: `README.md:51-53`

- [ ] **Step 1: Update the Security section**

Current line 53:
```markdown
NVIDIA OpenShell containers per agent, credential isolation, declarative network and filesystem policies, prompt injection detection. See [Security Model](docs/SECURITY.md) for the full picture.
```

Change to:
```markdown
NVIDIA OpenShell containers per agent, credential isolation, declarative network and filesystem policies, prompt injection detection. See [Security Model](docs/SECURITY.md) and [Policy Guide](docs/SECURITY.md#configuring-policies) for details.
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add policy guide anchor link to README"
```

---

### Task 9: Full workspace build and test

- [ ] **Step 1: Run full workspace build**

Run: `cargo build --workspace`
Expected: SUCCESS

- [ ] **Step 2: Run full workspace tests**

Run: `cargo test --workspace`
Expected: ALL PASS (ignoring `#[ignore]` tests)

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace`
Expected: No warnings
