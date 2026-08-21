# TLS interception is scoped by a bypass deny-list

Status: accepted (revised 2026-08-19 after live verification)

Right's Agent Sandbox intercepts TLS so that Provider credentials can be
substituted at the network boundary. The original decision claimed Right would
intercept *only* the hosts bound to a Provider and bypass everything else.
Live verification showed that shape does not exist upstream, so this record is
revised to describe the mechanism that does.

## Decision

Interception is controlled by a **bypass deny-list**, not an allowlist.
Configuring any Provider secret enables interception for every destination on
the intercepted ports — 443 by default — except destinations Right explicitly
bypasses. A secret's allowed-host list governs *substitution eligibility only*;
it does not narrow interception.

Right therefore:

- bypasses Anthropic's hosts, so the Agent's primary path is never intercepted
  and needs no trust configuration;
- treats the bypass list as a maintained, security-relevant inventory of
  uninspected destinations, reviewed when it changes;
- requires guest provisioning to install the guest's certificate package, so
  the interception CA composes with real root certificates instead of
  replacing them.

## Why the original shape is impossible

Upstream exposes interception as `enabled` plus a `bypass` list of exact hosts
and suffix patterns. There is no allowlist. Adding one secret flips
interception on globally. Verified live: with a secret present, a bypassed
Anthropic host was served the genuine upstream certificate while an unrelated
host was served the interception CA.

## Consequences

- **Claude traffic is unaffected.** Bypassed hosts terminate at the real
  origin with no CA configuration in the guest.
- **General Agent traffic is intercepted.** This is broader exposure than the
  original record claimed. Egress control is still the connection-level
  allow/deny policy and is independent of interception.
- **Guest trust must be provisioned deliberately.** The guest agent installs
  the interception CA into the guest trust directories and points the usual
  environment variables at a certificate bundle. On an image that already
  ships real roots this composes correctly. On an image without them, the
  guest agent creates a bundle holding *only* the interception CA — after
  which certificate-verifying clients trust the interception CA and nothing
  else, breaking precisely the **bypassed** hosts, Claude included.
  Provisioning installs the certificate package and never inherits or sets a
  client pinned to a private bundle.
- **Non-intercepted ports carry no substitution.** A placeholder sent to a
  destination outside the intercepted ports is forwarded unsubstituted rather
  than blocked. Provider destinations must be reachable over an intercepted
  port.
- **Certificate-pinning clients cannot be intercepted**, so they never receive
  a substitution.

## Evidence

`docs/superpowers/plans/2026-08-19-microsandbox-assumption-verdicts.md`,
assumption 3 and correction 2. Probes:
`crates/right-sandbox/tests/ci_msb_network.rs`.
