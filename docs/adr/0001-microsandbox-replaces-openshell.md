# microsandbox replaces OpenShell as the Sandbox Backend

Status: accepted

NVIDIA OpenShell is alpha software that breaks repeatedly, and its policy validator
structurally forbids the one shape Right needs: a permissive (whole-internet) Agent
Sandbox that also receives provider credentials. A hostless catch-all endpoint and a
composed per-provider endpoint conflict on `tls` and `allowed_ips`, so the whole policy
is rejected; an exhaustive 11-shape spike on a live sandbox found no working
combination and no upstream escape hatch. microsandbox provides the same primitives —
microVM isolation, deny-by-default egress with domain allowlists, TLS interception, and
host-side credential substitution — with substitution gated per secret on SNI + DNS pin
+ TLS identity + authority, independent of egress rules. Right therefore replaces the
backend outright rather than working around a defect in a dependency it does not
control.

## Considered options

- **Curated exact-host allowlist on OpenShell.** Proven to work, but permissive Agent
  Sandboxes would stop meaning "open web" — a product regression.
- **Forbid Providers on permissive Agents.** Fails loudly instead of degrading, but
  removes a feature users have.
- **File an upstream issue and wait.** OpenShell's per-host model is deliberate, not a
  bug; no issue or flag acknowledges the gap.

## Consequences

- No backend abstraction is introduced. Migration is an explicit one-time command, so no
  runtime dispatch between backends is ever needed, and `right-openshell` is deleted
  whole once migration completes.
- Sandboxless (`mode: none`) operation is removed. Every Agent has an Agent Sandbox.
- microVMs need KVM or Apple Silicon, which GitHub-hosted runners do not provide. Live
  sandbox tests lose CI coverage until self-hosted runners are provisioned.
- Right trades one beta dependency for another. Mitigation is an exact version pin, a
  startup version preflight, and a small real-VM contract suite.
- Credential substitution requires the credential to travel in a header; HTTP/2 DATA
  frames, compressed bodies, and bodies over 16 MiB are blocked rather than substituted.
