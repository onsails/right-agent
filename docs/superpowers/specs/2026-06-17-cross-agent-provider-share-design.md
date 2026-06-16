# Cross-agent provider sharing (multi-attach) — design

**Date:** 2026-06-17
**Status:** Draft — all decisions settled (one refcount sub-detail to confirm:
owner-deletion re-home, §Decisions.1). Ready to implement.
**Scope:** Replace the broken cross-agent provider *copy-by-readback* flow with
*multi-attach sharing*. Drop independent-copy. The secret stays in the gateway
and is never read back.

## Problem

Cross-agent provider "copy" (dashboard import/export → `handle_provider_copy` →
`get_provider_credentials` → write to destination) is fundamentally broken:

- OpenShell's `GetProvider` RPC **redacts** credential values — it returns the
  literal string `"REDACTED"`. The copy therefore writes `"REDACTED"` as the
  destination credential; the sandbox resolver substitutes it verbatim on
  egress → upstream **HTTP 401**. Verified:
  `crates/right-openshell/tests/ci_openshell_get_provider_redacts.rs`. Full
  trail: `/tmp/provider-copy-investigation-handoff.md` §14.
- There is **no host-side gRPC path to read a real credential back**
  (`GetProvider`/`ListProviders` redact; `GetSandboxProviderEnvironment`
  requires a sandbox principal the host can't present — verified empirically).
  So copy-by-readback cannot be fixed; it fights OpenShell's design.

Meanwhile **multi-attach works**: one provider record attached to N sandboxes
resolves the real secret on each, with no read-back. Verified:
`crates/right-openshell/tests/ci_openshell_provider_multi_attach.rs`.

Two distinct needs were conflated by "copy". Per product decisions only one is
kept:
- **Share the same account** across agents (e.g. one fal account for `right` and
  `agent-a`) → multi-attach. **Keep.**
- **Independent copy** (separate records, independent rotation) → needs the
  secret value, unreadable from the host → **dropped** (decision §Decisions.3).

## Solution

A provider record is a free-standing gateway object whose **name carries no
agent reference**. "**Share** provider P with agent B" attaches the *same*
gateway record to B's sandbox and records a *borrowed* reference in B's
`agent.yaml`. No secret moves; no read-back.

### Naming — decouple record identity from the owning agent (decision §Decisions.4)

Today records are named `{agent}-{slug}`, conflating identity with ownership and
making a shared record look like it belongs to one user. New records use an
**agent-agnostic** name `{type-slug}-{short-uuid}` (e.g. `fal-a1b2c3`). The slug
keeps `openshell provider list`/logs readable (debuggability convention); the
name is never surfaced in the UI (display goes through `display_name`/`label`).

Constraints (load-bearing):
- **Existing `{agent}-{slug}` records cannot be renamed** — the OpenShell name is
  the key, rename = delete+recreate, recreate needs the credential, which is
  unreadable (redaction). So the new scheme applies to NEW records only; existing
  records keep their names. Both forms MUST be supported (backward-compat;
  upgrade-friendly — no recreation).
- **Ownership is no longer encoded in the name → it moves into `agent.yaml`
  (explicit data):** the OWNER is the agent that declares the record without a
  `shared_from` marker; borrowers declare it with `shared_from: <owner>`.
- **`validate_name`** drops the `{agent}-` requirement for new records (accepts
  the `{slug}-{uuid}` form) while still accepting legacy `{agent}-{slug}`.

- **Connected rotation (decision §Decisions.2 — confirmed):** rotating the owner's
  record updates every borrower's sandbox on the next resolver refresh (~10s).
  The dashboard MUST show this explicitly: a borrowed provider is read-only for
  the borrower and labeled "shared from `{owner}` — credential & rotation
  controlled by the owner".
- **Trust:** sharing requires the actor trusted in BOTH agents' allowlists (same
  rule as today's copy — ARCHITECTURE.md "Providers").
- **Lifecycle (decision §Decisions.1 — REFCOUNT):** a record lives as long as ANY
  agent references it (declares it in `agent.yaml`). Deleting an agent detaches
  the record from its sandbox and removes its entry; the gateway record is
  `delete_provider`'d **only when no other agent references it** (refcount → 0).
  This is symmetric — deleting owner or borrower is the same logic, no special
  case. If the deleted agent was the OWNER and borrowers remain, **re-home**
  ownership to a surviving referencer (clear its `shared_from`, repoint the others'
  `shared_from` to the new owner) so rotation stays controllable
  (§Decisions.1 sub-detail).

### Data model (`agent.yaml`)

Ownership is recorded in `agent.yaml`, not in the name. Owner's entry has no
`shared_from`; borrower's entry references the same record id and marks it
borrowed so reconcile/UI never try to own/rotate/delete it:

```yaml
# owner (agent-a)
sandbox:
  providers:
    - name: fal-a1b2c3             # agent-agnostic record id

# borrower (right)
sandbox:
  providers:
    - name: fal-a1b2c3             # SAME record id
      shared_from: agent-a         # presence ⇒ borrowed; read-only for this agent
```

`shared_from: None` (absent) = owned. Backward-compatible additive field
(`MergedRMW` agent.yaml; existing `{agent}-{slug}` entries unchanged and still
owned). Legacy records keep their names; only their ownership is now read from
"declared-without-`shared_from`" rather than inferred from the prefix.

### Reconcile

`reconcile_for_sandbox` already **attaches any declared name regardless of prefix**.
Its **detach** guard, however, currently keys on the `{agent_prefix}-` name prefix
(`name.starts_with(prefix) && !declared`) to avoid stripping another agent's
provider. With agent-agnostic names that heuristic no longer works, so detach must
switch to **`agent.yaml` as the sole ownership signal**: detach a provider that is
attached to this sandbox but NOT in this agent's declared list — except never
detach the agent's own **borrowed** entries via a foreign mechanism (they are
declared, so they're kept). Required changes:
- Detach decision: attached-but-not-declared (per this agent's `agent.yaml`),
  independent of name prefix. (Legacy prefixed names still parse; ownership comes
  from the declared list, not the prefix.)
- Borrowed generic providers: ensure the owner's managed profile exists before
  attach (reuse `managed_profiles::ensure_*`); do NOT re-import/own it.
- Never run owner-only repair (`legacy_generic_provider_recreate_payload`) on a
  borrowed (`shared_from`) entry.
- `hot_reconcile_providers` (bot) treats `shared_from` entries as attach-only.
- `destroy_agent` cascade (`right-agent/src/agent/destroy.rs`) iterates this
  agent's `sandbox.providers` and `delete_provider(name)` per entry. Under
  **refcount** it MUST: always **detach** from this sandbox; `delete_provider`
  **only when no other agent references the record** (scan other agents'
  `agent.yaml` for the same record id). If the deleted agent owned a record still
  referenced by borrowers → keep the record and **re-home** ownership to a
  surviving referencer. Without the refcount check, deleting EITHER agent deletes
  the shared record (verified: destroy deletes by name from the agent's own
  agent.yaml).

### Control plane (dashboard / internal API)

- New internal routes: `provider_share { actor, owner_agent, provider, dest_agent }`
  and `provider_unshare { actor, borrower_agent, provider }`. Both go through the
  Unix-socket internal API (ARCHITECTURE.md dashboard write contract), resolve
  trust on both sides, then attach/detach + write the borrower's `agent.yaml`.
- Remove (or repoint) the copy-by-readback import/export routes. Keep the
  `SourceCredentialUnreadable` guard (already shipped) as defense-in-depth for any
  residual read-back path.
- `/providers` UI: per-owned-provider "Share with…" (trusted peers only) and
  per-borrowed-provider "Unshare"; borrowed shown read-only with owner label.

## Non-goals

- Independent per-agent credential copies (dropped).
- Any host-side secret read-back.
- Renaming/re-homing records (owner keeps the record; borrowers reference it).

## Decisions

1. **Lifecycle — REFCOUNT (CONFIRMED this session).** A record lives while any
   agent references it; deletion `delete_provider`s only at refcount 0. Symmetric
   for owner/borrower. **Sub-detail to confirm:** on owner deletion with surviving
   borrowers, re-home ownership to a survivor (PROPOSED) vs leave the record
   owner-less (dangling `shared_from`, rotation impossible until re-shared).
2. **Connected rotation — CONFIRMED.** Owner rotation propagates to borrowers;
   surface explicitly in UI.
3. **Independent copy — DROPPED.** Simplify; only sharing remains.
4. **Agent-agnostic record names — CONFIRMED (this session).** New records:
   `{type-slug}-{short-uuid}`; ownership recorded in `agent.yaml`, not the name.
   Existing `{agent}-{slug}` records keep their names (unrenamable — recreate
   needs the unreadable credential) and stay owned by the declaring agent. Both
   forms supported. This is the foundation that makes ownership explicit data and
   the lifecycle a clean policy.

## Implementation plan (TDD cadence)

Baseline: at worktree start run `cargo nextest run -p right-openshell -p right`
and record pre-existing failures (note: workspace `cli_integration`/`wizard_brand`/
`home_isolation` currently fail on a leftover cloudflared tunnel — environmental).

1. **agent.yaml schema + naming:** add `shared_from: Option<String>` to the
   provider entry type in `right-agent-config`; relax `validate_name` to accept
   the new `{type-slug}-{short-uuid}` form AND legacy `{agent}-{slug}`; add the
   id generator for new records. Unit-test (de)serialization round-trip incl.
   backward-compat (absent field) + both name forms validating. *Targeted:*
   `cargo nextest run -p right-agent-config -p right`.
2. **Ownership = declared list, not prefix:** switch `reconcile_for_sandbox`
   detach from `{agent_prefix}-` to attached-but-not-declared (per agent.yaml);
   borrowed (`shared_from`) entries are attach-only, profile-ensure without
   import, never owner-repaired. *Targeted:* `cargo nextest run -p right-openshell`
   + a live `ci_openshell_` test that declares a borrowed record and asserts it
   stays attached + resolves across a reconcile pass, and that an undeclared
   attached record is detached.
3. **Internal API `provider_share` / `provider_unshare`:** trust-on-both, attach/
   detach, agent.yaml write (owner entry vs `shared_from` borrower entry); reject
   sharing into self; reject unshare of an owned record. New records created by
   the existing add-provider path now get a `{type-slug}-{uuid}` name. Unit-test
   the planning/trust logic. *Targeted:* `cargo nextest run -p right`.
4. **Deletion safety — refcount (decision 1):** `destroy_agent` cascade always
   detaches, but `delete_provider`s a record **only when no other agent's
   agent.yaml references it**; if the deleted agent owned a still-referenced
   record, re-home ownership to a survivor. Unit-test: owner-delete-with-borrower
   keeps the record + re-homes; last-referencer-delete deletes the record.
   *Targeted:* `cargo nextest run -p right -p right-agent`.
5. **Dashboard routes + Vue UI:** Share/Unshare actions; borrowed read-only label
   via existing primitives (`AsyncState`, `identityLabels`); SSR component tests.
   *Targeted:* dashboard tests.
6. **Retire copy-by-readback:** remove/redirect import/export-by-readback; keep the
   `SourceCredentialUnreadable` guard. Update `ci_openshell_get_provider_redacts`
   reference if needed.
7. **Docs:** update ARCHITECTURE.md "Providers" (copy-only → share), the providers
   satellite doc, and PROMPT_SYSTEM.md if any agent-facing tool text changes.

**Final verification (mandatory):** from the worktree, `cargo nextest run
--workspace` + `cargo test --doc --workspace`, plus the new `ci_openshell_` share
test run explicitly with `--run-ignored all`. Targeted tests do not replace the
final full workspace run.

## Upgrade & migration

Additive `shared_from` defaults to owned behavior → already-deployed agents are
unaffected until an operator shares. No sandbox recreation; no `right agent init`.
Borrowed attachments self-heal on bot startup reconcile (re-attach if missing).
```
