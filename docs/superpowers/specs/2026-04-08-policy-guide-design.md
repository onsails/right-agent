# Policy Guide & Network Policy Choice Design Spec

## Goal

1. Add a network policy choice during `rightclaw init` — restrictive (Anthropic/Claude only) vs permissive (all HTTPS)
2. Add a "Configuring Policies" section to `docs/SECURITY.md` explaining how to customize agent policies
3. Update README.md Security section to link directly to the new section

## Deliverables

1. **Code changes** — `NetworkPolicy` enum, init prompt, policy codegen branching
2. **Doc changes** — new section in SECURITY.md, anchor link in README.md

## Code Changes

### 1. `crates/rightclaw/src/agent/types.rs`

Add `NetworkPolicy` enum to `AgentConfig` (not inside `SandboxOverrides` — it's a top-level agent concern):

```rust
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// Only allow Anthropic/Claude domains (*.anthropic.com, anthropic.com, *.claude.com, claude.com, *.claude.ai)
    Restrictive,
    /// Allow all outbound HTTPS
    #[default]
    Permissive,
}
```

Add field to `AgentConfig`:
```rust
#[serde(default)]
pub network_policy: NetworkPolicy,
```

Default is `Permissive` for backwards compatibility with existing agent.yaml files that don't have this field.

### 2. `crates/rightclaw/src/codegen/policy.rs`

Change `generate_policy` signature:
```rust
pub fn generate_policy(right_mcp_port: u16, network_policy: &NetworkPolicy) -> String
```

When `NetworkPolicy::Restrictive`, generate the network_policies section with explicit domain list:
```yaml
network_policies:
  anthropic:
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
      - path: "**"
```

When `NetworkPolicy::Permissive`, keep current `**.*` wildcard behavior.

### 3. `crates/rightclaw/src/init.rs`

Add `prompt_network_policy()` function, similar to existing `prompt_telegram_token()`:
```
Network policy for sandbox:
  1. Restrictive — Anthropic/Claude domains only (recommended for production)
  2. Permissive — all HTTPS domains allowed (needed for external MCP servers)

Choose [1/2] (default: 1):
```

Default: restrictive (1).

Update `init_rightclaw_home` signature to accept `NetworkPolicy`. Write `network_policy: restrictive` or `network_policy: permissive` into agent.yaml.

### 4. `crates/rightclaw-cli/src/main.rs`

Add `--network-policy <restrictive|permissive>` CLI flag to `Init` command. When not provided and interactive, call `prompt_network_policy()`. When `--yes` flag is set, default to restrictive.

### 5. All callers of `generate_policy`

Update all call sites to pass the agent's `NetworkPolicy`. Currently called from:
- `cmd_up` in main.rs (has access to `AgentConfig`)
- Bot startup in `bot/src/lib.rs` (needs to read from agent config or env var)

### 6. Template `templates/right/agent.yaml`

Add commented example:
```yaml
# network_policy: restrictive  # restrictive = Anthropic/Claude only, permissive = all HTTPS
```

## Documentation Changes

### 1. `docs/SECURITY.md` — new "Configuring Policies" section

Add after the existing "Declarative Policies" section. Content:

**Default behavior:**
- By default (`network_policy: permissive`), agents can reach any HTTPS endpoint. All traffic goes through OpenShell's proxy with TLS termination for inspection, but no domain restrictions.
- With `network_policy: restrictive`, only Anthropic/Claude domains are allowed (*.anthropic.com, anthropic.com, *.claude.com, claude.com, *.claude.ai).

**Choosing during init:**
- `rightclaw init` prompts for network policy choice
- Can also set via `--network-policy restrictive|permissive`

**Changing after init:**
- Edit `agent.yaml` and set `network_policy: restrictive` or `network_policy: permissive`
- Run `rightclaw up` to regenerate and apply

**Custom domain allowlists:**
- For fine-grained control beyond restrictive/permissive, edit the generated policy directly at `~/.rightclaw/run/policies/<agent>.yaml`
- Add endpoint entries to the `network_policies` section following OpenShell format
- Caveat: `rightclaw up` regenerates this file — manual edits are overwritten
- For persistent custom policies, edit the generated file after each `rightclaw up` (policies are regenerated on every launch)

**Example: adding a specific domain to restrictive policy:**
```yaml
# In ~/.rightclaw/run/policies/right.yaml, add under network_policies:
  my_mcp_server:
    endpoints:
      - host: "mcp.notion.com"
        port: 443
        protocol: rest
        access: full
        tls: terminate
    binaries:
      - path: "**"
```

### 2. `README.md` — update Security section

Change:
```
See [Security Model](docs/SECURITY.md) for the full picture.
```
To:
```
See [Security Model](docs/SECURITY.md) and [Policy Guide](docs/SECURITY.md#configuring-policies) for details.
```

## Constraints

- Default `NetworkPolicy` is `Permissive` for backwards compat with existing agent.yaml files
- Init default prompt is `Restrictive` (recommended for new agents)
- No auto-adding of MCP server domains in restrictive mode — users edit policy manually
- Don't repeat OpenShell policy format docs — just show where to edit and one example
