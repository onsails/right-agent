# TLS interception is scoped to Provider hosts only

Status: accepted

microsandbox can intercept and re-sign all TLS from an Agent Sandbox, but Right
intercepts only the hosts bound to a Provider secret and bypasses everything else.
Interception exists to serve credential substitution; intercepting traffic Right does not
act on buys inspection nobody consumes and costs a per-runtime trust matrix.

## Considered options

- **Intercept broadly.** The interception CA is installed into the guest *system* trust
  store, which covers curl, apt, and Python. Node does not read that store by default,
  and Claude Code is Node — so broad interception would require injecting
  `NODE_EXTRA_CA_CERTS`, and a new entry for every future runtime with its own trust
  policy (Go, Rust with webpki-roots). Certificate-pinning clients cannot be intercepted
  at all.

## Consequences

- Claude Code's own traffic to Anthropic is never intercepted, so no trust configuration
  is needed for the agent's primary path.
- The credential-exposure surface is the Provider host set and nothing else.
- Right gains no L7 visibility into general Agent traffic. Egress control remains the
  connection-level allow/deny policy, which is unaffected by interception scope.
