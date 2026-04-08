# Security Model

RightClaw enforces security at the infrastructure level. Every agent runs inside an isolated container with declarative policies — not through permission prompts or trust-based configuration.

## Sandbox Architecture

Each agent runs inside its own [NVIDIA OpenShell](https://github.com/NVIDIA/OpenShell) sandbox — a k3s container managed via gRPC. Sandboxes are persistent (survive bot restarts) and isolate:

- **Filesystem** — agents can only access paths explicitly allowed by policy
- **Network** — all traffic routes through an HTTPS proxy (`10.200.0.1:3128`) with domain allowlists
- **Credentials** — each sandbox has its own authentication state, independent of the host
- **Processes** — agent processes are contained within the sandbox boundary

Sandboxes are Docker containers. Back them up, snapshot them, migrate them — standard container operations apply.

## Credential Isolation

Host credentials (`.credentials.json`) are **never** uploaded to sandboxes. Each agent authenticates independently through an OAuth login flow that happens entirely inside the sandbox. The login flow is PTY-driven and managed through Telegram — the user receives an OAuth URL, clicks it, and pastes the auth code back in chat.

MCP OAuth tokens are stored per-agent and refreshed automatically (10 minutes before expiry). Token refresh happens on the host and the updated `.mcp.json` is uploaded to the sandbox.

## Network Policy

All sandbox network traffic goes through OpenShell's HTTPS proxy:

- **Domain allowlists** — wildcard patterns (e.g., `*.anthropic.com`, `*.claude.ai`) control which endpoints agents can reach
- **TLS termination** — the proxy terminates and re-signs TLS with a per-sandbox CA for L7 inspection. Required on all HTTPS endpoints (OpenShell v0.0.23+).
- **Policy hot-reload** — network rules can be updated without restarting the sandbox via `openshell policy set --wait`

## Declarative Policies

Each agent gets a generated policy file controlling:

- **Filesystem rules** — read/write paths, binary execution paths
- **Network rules** — allowed domains, allowed IPs, TLS termination settings
- **Binary restrictions** — which executables the agent can run (`path: "**"` for full access, or locked down per-binary)

Policies are regenerated on each `rightclaw up` from `agent.yaml` configuration and sandbox override settings.

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

## Prompt Injection Guard

The memory store (SQLite) runs incoming content through pattern matching based on OWASP prompt injection vectors before insert. Detected injection attempts are rejected before they reach the database.

## Access Control

- **Chat ID allowlist** — each agent has a per-agent list of allowed Telegram chat IDs. Empty list = block all (secure default).
- **Protected MCP servers** — the built-in "right" MCP server cannot be removed via `/mcp remove`
- **OAuth CSRF protection** — token matching in the OAuth callback server prevents cross-site request forgery

## Compliance

RightClaw calls `claude -p` directly, using your existing Claude subscription. There is no token arbitrage, no API key sharing, and no man-in-the-middle on Claude's authentication. This makes RightClaw fully compliant with Anthropic's Terms of Service.
