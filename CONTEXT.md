# Context

Domain glossary for Right Agent. Glossary only — implementation decisions live in `docs/adr/`.

## Agent

An opinionated, closed-box AI agent instance. One Telegram bot per Agent; every chat its own Claude Code session.

## Agent Sandbox

The isolated execution environment of one Agent: a persistent, named microVM. Every Agent has exactly one. Sandboxless (on-host) operation does not exist.

## Sandbox Backend

The runtime that creates and runs Agent Sandboxes. OpenShell is the legacy backend; microsandbox is the current backend. Selected at the platform level, not per Agent.

## Egress Mode

Per-Agent network stance. **Permissive**: public internet allowed. **Restrictive**: explicit destination allowlist only. Independent of credential substitution.

## Provider

An external API credential plus the endpoint binding that scopes where it may be sent. Credentials live on the host; the Agent Sandbox sees only a placeholder.

## Provider Owner

The Agent whose configuration created a Provider. The owner may rotate, edit, and delete it.

## Borrowed Provider

A Provider referenced by an Agent that does not own it. Borrowing grants use, never mutation. Ownership transfers to a surviving borrower if the owner is destroyed.

## Provider Status

**Ready**: credential present and the Agent Sandbox is running. **Needs-value**: binding exists, credential absent. **Error**: anything else.

## Credential Substitution

Replacement of a placeholder with the real credential at the sandbox network boundary, only for destinations bound to that Provider. Works in both Egress Modes.

## Sandbox Migration

The one-time move of an Agent Sandbox from the OpenShell backend to the microsandbox backend: agent-owned filesystem content carried over, platform-owned files regenerated.
