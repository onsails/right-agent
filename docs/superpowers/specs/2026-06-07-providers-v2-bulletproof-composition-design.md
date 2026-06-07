# Design: Bulletproof providers-v2 composition

> **Status:** approved design (2026-06-07). Successor to the discussion doc
> `docs/providers-v2-composition-handoff.md`. Code is authoritative; verify
> line numbers before editing — they drift.

## Context

OpenShell v0.0.56 gates provider-profile network-endpoint composition behind a
gateway-global runtime setting `providers_v2_enabled` (**default `false`** on
fresh gateways). With the flag off, providers attach and the credential
placeholder env var is injected, but the proxy denies CONNECT
(`403 policy_denied`) because the terminated upstream endpoint is never composed
into the sandbox's effective policy.

A proto refresh (`31353cff`, 2026-05-28) removed Right's `ensure_v2_enabled`
call on the false belief that v0.0.50+ enabled providers unconditionally. The
fix `9a7aad43` (2026-06-06) restored it in `right up`.

### What the design audit found

Git archaeology + a full inventory of provider workarounds established:

- The dev (macOS) gateway held `providers_v2_enabled = true` via **persisted
  state** from the pre-removal code, invisibly, throughout subsequent design
  work. So every provider design decision from 2026-05-28 onward was validated
  against an **invisibly-enabled** v2 — not against the production default.
- The decisive regression is **not** "hacks that compensate for v2-off
  brokenness" (there are none — the gateway was on). It is that the
  fold→composition migration (`8acd91dc`, 2026-06-05) **discarded folding**, the
  only v2-*independent* mechanism (writing the terminated endpoint directly into
  `policy.yaml`, proven to work on Linux by the repro positive control), in
  favour of composition, which works **only** when v2 is on. That trade was
  validated only because the dev gateway still had v2 persisted on.
- The blast radius is wider than generic providers: **all** provider credential
  substitution — built-in (`right-github`, etc.) *and* generic — relies on the
  gateway composing provider-profile endpoints into the effective policy. The
  whole feature rides v2.
- Workarounds designed under v2-on remain **correct** once v2 is guaranteed; the
  genuinely fragile piece is the indirect "loaded signal" (`policy set --wait`,
  which no-ops on an unchanged policy hash and then does not notify the watch
  bus, so composition may not re-pull).

### Decision

Keep composition as the single mechanism ("we need providers v2"). Make
v2-enablement guaranteed on every provider-attach path, and make composition
**success directly observed** rather than inferred from a side effect. Do not
restore folding.

## Goals

1. `providers_v2_enabled` is guaranteed true on every path that attaches a
   provider — `right up`, dashboard `/providers`, and config-watcher
   hot-reconcile — not just `right up`.
2. Composition success is confirmed by reading the active policy, so a
   "flag set but composition didn't happen" divergence fails loudly instead of
   shipping as a silent upstream 401.
3. A future new attach path that forgets to enable v2 fails **loudly** (timeout,
   not silent 401) without relying on a global memo.
4. Operators can see per-provider composition state (composed / not composed).

## Non-goals

- Restoring policy folding (`apply_provider_stanzas` / `# right-providers`
  anchor). Folding stays removed; the legacy folded-policy strip path stays as
  cleanup-only and is untouched.
- Adding a proto `GetConfig`/`GetSetting` RPC for flag read-back. Not available
  in v0.0.56 (`UpdateConfigResponse` carries only `version`, `policy_hash`,
  `settings_revision`, `deleted` — not the stored value), and unnecessary:
  confirming the composed `_provider_<name>` rule in the active policy is
  strictly stronger than reading the flag.
- Changing the cross-provider credential-confinement limitation
  (`onsails/right-agent#92`) or any other v2-independent workaround.

## Design

### Component 1 — Guarantee v2 at the two real attach funnels

`ensure_v2_enabled` (`right_openshell::providers`, gRPC `UpdateConfig` global
bool upsert) is called:

- At the top of `sandbox_supervisor::reconcile_for_sandbox` — covers both
  supervisor paths (bring-up and `hot_reconcile_providers`).
- In the dashboard provider create/attach handler(s) in
  `internal_api_providers.rs` (the path that bypasses `reconcile_for_sandbox`).
- `cmd_up` keeps its existing early call (loud fail-fast at startup).

One idempotent upsert per provider-management operation — cheap, no per-process
memo, no gateway-endpoint keying (that keying would break test isolation and is
overengineering given Component 2 is the universal backstop).

**Error semantics per path:**

| Path | On `ensure_v2_enabled` failure |
|------|-------------------------------|
| Dashboard add | Hard, surfaced error to the user — the add cannot work without it. |
| hot-reconcile | Fatal/log when a provider is declared; retry next reconcile tick. Tolerate when none declared. |
| `cmd_up` | Unchanged: fatal when any agent declares providers, warning-only otherwise. |

### Component 2 — Direct composition confirmation (the centerpiece)

New helpers, `wait_for_provider_composed(client, sandbox_name, provider_name)`
and the generic-specific endpoint-aware variant in `right_openshell`:

- After `attach_to_sandbox` + `ensure_provider_policy_loaded`, poll
  `get_active_policy` (host-callable via `GetSandboxPolicyStatus`) until the
  composed rule `_provider_<sanitized-name>` appears in
  `SandboxPolicy.network_policies`.
- For generic provider create/config-update and supervisor reconciles, also
  require the composed rule to contain the expected upstream host/path from the
  authored generic profile. A pre-update rule for the same provider name is not
  a fresh composition signal.
- Reuse the existing matcher `provider_capabilities::rule_for_provider` (the
  `_provider_` prefix + `sanitize` logic already lives there).
- Bounded timeout; on timeout return an error (FAIL FAST) with diagnostics (the
  attached-but-uncomposed state).
- `policy set --wait` (`ensure_provider_policy_loaded`) stays as the recompose
  **trigger**; the **success signal** becomes active-policy rule/content
  presence, not the policy-set return value. This closes the "no-op on
  unchanged hash → no notify → composition not re-pulled" hole.

This aligns with the project convention "debuggability over convenience: use the
direct signal, do not infer status from side effects."

### Component 3 — Universal backstop (falls out of Component 2)

Because every attach is followed by `wait_for_provider_composed*`, a future
attach path that forgets `ensure_v2_enabled` fails loudly: the `_provider_` rule
or expected generic endpoint never appears, the poll times out, and the
operation errors — instead of a silent upstream 401. This is what makes Option A
bulletproof without a global memo or a structural invariant inside
`attach_to_sandbox`.

### Component 4 — Observability

The composed/not-composed state is already computed as the `active` flag in
`provider_capabilities::correlate_provider_capabilities` (and surfaced in
`usage_hint`). Expose it:

- In the dashboard per-provider status (composed: yes/no).
- Log it at attach time (host log; never log placeholder/credential values).

### Component 5 — Docs and invariants

- `ARCHITECTURE.md` (provider section): add the rule "every provider attach
  path guarantees `providers_v2_enabled`; composition success is confirmed by
  reading the `_provider_<name>` rule in the active policy, never by the
  `policy set` return value."
- `docs/architecture/providers.md`: document `wait_for_provider_composed`, the
  two-funnel v2 guarantee, and the loud-backstop property. Note the scope
  (built-in + generic both ride composition).
- Folding stays removed; legacy strip untouched.

## Testing strategy

TDD, narrowest-first (per `AGENTS.rust.md`):

1. **Unit — `wait_for_provider_composed`:** drive a fake `SandboxPolicy` with
   and without the `_provider_<name>` rule; assert success when present, timeout
   error when absent. Pure matcher reuse means most logic is already covered by
   `provider_capabilities_tests.rs`.
2. **Unit — funnel v2 guarantee:** with the mock gateway (`test_mock_server.rs`
   configurable `update_config`), assert each funnel (`reconcile_for_sandbox`,
   dashboard create/attach) invokes `ensure_v2_enabled` before attach. Assert
   per-path error semantics (dashboard hard error; hot-reconcile retry).
3. **Integration (live, `ci_openshell_` / `ci_*` prefixed, called by the
   ignored-test CI filter):** on the Linux gateway container with
   `providers_v2_enabled` reset to **false**, a dashboard-style add must
   self-enable v2, compose, and `wait_for_provider_composed` must pass +
   substitution must succeed. Extend the existing
   `ci_openshell_generic_provider.rs` / `ci_openshell_github.rs` to assert the
   `_provider_` rule is present (not just that CONNECT succeeds).

## Verification cadence

- Targeted package tests during the red/green loop
  (`devenv shell -- cargo test -p right-openshell <filter>`, `-p bot`, `-p right`).
- Live re-validation on the Linux gateway container (recipe in the handoff doc)
  with the flag forced to false.
- Final mandatory `devenv shell -- cargo test --workspace` before declaring
  complete.

## Risks / open questions

- **`wait_for_provider_composed` timeout value.** Composition is
  eventually-consistent (sub-second for attach per `providers.md`). Pick a
  bounded timeout with margin (e.g. a few seconds) and surface the
  attached-but-uncomposed diagnostic on timeout. Tune against the live container.
- **Built-in providers and the `_provider_` rule key.** Confirm built-in
  providers (e.g. `right-github`) compose under a `_provider_<name>` rule with
  the same key shape `rule_for_provider` expects; if built-ins use a different
  key, the matcher must handle both. Verify on the live container before relying
  on the backstop for built-ins.
- **hot-reconcile retry interaction.** Ensure a `wait_for_provider_composed`
  timeout in `hot_reconcile_providers` routes through the existing bounded
  backoff rather than wedging the supervisor.

## References

- `docs/providers-v2-composition-handoff.md` — prior discussion, root cause,
  Linux gateway repro recipe.
- `crates/right-openshell/src/providers.rs` — `ensure_v2_enabled`,
  `PROVIDERS_V2_ENABLED_KEY`, `attach_to_sandbox`, `reconcile_for_sandbox`.
- `crates/right-openshell/src/provider_capabilities.rs` — `rule_for_provider`,
  `_provider_` prefix, `active` flag (reused by Component 2 & 4).
- `crates/right-openshell/src/openshell.rs` — `get_active_policy`
  (`GetSandboxPolicyStatus`), `ensure_provider_policy_loaded`.
- `crates/bot/src/sandbox_supervisor.rs` — `reconcile_for_sandbox`,
  `hot_reconcile_providers`.
- `crates/right/src/internal_api_providers.rs` — dashboard provider add (P2 target).
- `crates/right/src/main.rs` `cmd_up` — existing `ensure_v2_enabled` call.
- `docs/architecture/providers.md` — provider subsystem (documents the v2 gate).
- Memory: `project_openshell_gateway_state_masks_regressions`.
