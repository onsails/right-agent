# Handoff: `providers_v2_enabled` composition gap & provider-design implications

> **Status:** discussion doc for next session. Written 2026-06-07, after the
> `providers_v2_enabled` regression was root-caused and fixed (commit
> `9a7aad43`). Self-contained — a fresh session should not need the prior
> transcript. Code is authoritative; verify line numbers before editing.

## TL;DR

A real **Linux production regression** (not a flaky CI test) was found and
fixed: OpenShell v0.0.56 gates provider-profile network-endpoint composition
behind a gateway-global runtime setting `providers_v2_enabled` (**default
false** on fresh gateways). A proto refresh had removed Right's
`ensure_v2_enabled` call on the false belief that v0.0.50+ enabled it
unconditionally. With the flag off, providers attach and the credential
placeholder env var is injected, but the proxy **denies CONNECT**
(`403 policy_denied`) because the terminated upstream endpoint is never
composed into the sandbox's effective policy. macOS dev gateways had the flag
persisted from a prior enable, masking the break locally while CI/Linux/new
prod hosts silently failed.

**Landed fix (`9a7aad43`):** restored `right_openshell::providers::ensure_v2_enabled`
(gRPC `UpdateConfig` global upsert) and call it in `right up` (`cmd_up`) +
in the provider integration tests. CI is green.

**This doc is for deciding three things:**
1. **P2 (recommended to implement):** the fix only covers `right up`. Live
   provider-add paths (dashboard `/providers`, config-watcher hot-reconcile)
   do **not** ensure the flag → same bug, just outside `right up`. Where and
   how to close this.
2. **P3 (discussion):** our load-bearing "rely on gateway composition, never
   fold provider endpoints into `policy.yaml`" decision is intact but now
   visibly fragile. Keep, or move folding from rejected to fallback?
3. **P4 (discussion):** defense-in-depth so a future "flag set but composition
   silently didn't happen" divergence is caught, not shipped.

---

## Root cause (full mechanism, OpenShell v0.0.56)

OpenShell gateway is open source (NVIDIA/OpenShell, Apache-2.0). At tag
`v0.0.56`:

- **Composition is gated + pull-based.** `compose_effective_policy`
  (`crates/openshell-policy/src/compose.rs`) merges attached-provider
  `ProviderPolicyLayer` endpoints into a sandbox's effective policy. It is
  called **only** inside the gateway's `GetSandboxConfig` RPC handler
  (`crates/openshell-server/src/grpc/policy.rs`), i.e. when the in-sandbox
  supervisor *pulls* its effective policy — and **only if**
  `bool_setting_enabled(global_settings, PROVIDERS_V2_ENABLED_KEY)` is true.
  `defaults.rs` does not set a default → effectively **false** on a fresh
  gateway. Flag off ⇒ provider layers silently skipped, no error.
- **`AttachSandboxProvider` does not trigger recomposition.** It CAS-writes
  `sandbox.spec.providers` and returns. It does **not** notify the
  `sandbox_watch_bus`. Only `UpdateConfig` (i.e. `openshell policy set`)
  notifies the bus, which is what makes the supervisor re-pull `GetSandboxConfig`.
  → This is why Right's `ensure_provider_policy_loaded` (re-apply base policy
  via `openshell policy set --wait`) after attach is load-bearing: it is the
  refetch barrier. Caveat observed in repro: `policy set` is a **no-op when the
  policy hash is unchanged** ("Policy unchanged") and then does not notify —
  a latent fragility, though the real flow works (CI green), so the hash
  differs enough in practice or the supervisor refetches on reconnect.
- **The flag persists** in the gateway's settings store. A long-lived dev
  gateway enabled once stays enabled — which is exactly why the regression was
  invisible on macOS and only bit fresh Linux gateways.

**Decisive evidence:** running Right's *real gRPC test path* against a
`ghcr.io/nvidia/openshell/gateway:0.0.56` Linux container — flag off → deny;
`openshell settings set --global --key providers_v2_enabled --value true` →
substitution works (header substituted). The positive control (writing the
terminated endpoint directly into the base `policy.yaml`) also works on Linux,
proving the proxy/L7-termination/substitution machinery itself is fine — only
provider-profile→policy composition was missing.

L7 CONNECT-gate detail (for future debugging): the OPA rule
(`crates/openshell-sandbox/data/sandbox-policy.rego`) allows a CONNECT iff one
policy entry satisfies both `endpoint_allowed` AND `binary_allowed`.
`endpoint_allowed` does exact-string host match (no CIDR expansion — a base
`host: "0.0.0.0/0"` literal matches *nothing*; hostless `allowed_ips` entries
match any host on the port). `binary_allowed` matches the connecting process's
resolved binary path (via Linux procfs) against the endpoint's `binaries`
globs; `**` matches any path. So substitution requires the *provider* entry
(host-matched) to carry binaries that admit the agent binary, which
`author_generic_profile` does (`binaries: [{path:"**"}]`).

---

## What landed (`9a7aad43`)

- `crates/right-openshell/src/providers.rs`: `pub const PROVIDERS_V2_ENABLED_KEY`
  + `pub async fn ensure_v2_enabled(client) -> Result<(), ProviderError>`
  (gRPC `UpdateConfig { global: true, setting_key, setting_value: bool(true) }`,
  FAIL FAST). Unit-tested against the mock (`providers_tests.rs`,
  `test_mock_server.rs` `update_config` now configurable).
- `crates/right/src/main.rs` (`cmd_up`): calls it once, in the existing
  `if any_sandboxed { connect_grpc(...) }` provisioning block, before profile
  provisioning / sandbox bring-up. **Fatal** when any agent declares providers,
  **warning-only** otherwise.
- `crates/right-openshell/tests/ci_openshell_generic_provider.rs` (all 3) +
  `ci_openshell_github.rs` (provider test): call `ensure_v2_enabled` after
  `connect_grpc`, before attach (they bypass `right up`).
- `docs/architecture/providers.md`: documents the v2 gate.

Also in this session's master (context, not part of this fix): STT
partial-download race fix + `right stt preload`/CI prefetch, clippy gate
clear, and a revert of a useless 90s diagnostic poll. The provider test
retains failure diagnostics (`connect_failure_diagnostics`, `RAW_CONNECT_PROBE`,
composed-policy dump) — keep them; they make this class of failure
self-diagnosing.

---

## P2 — the coverage gap (recommended to implement next session)

**Problem.** `ensure_v2_enabled` is called from exactly one non-test site:
`crates/right/src/main.rs` (`cmd_up` = `right up`). Verified:
`rg "ensure_v2_enabled" crates/ --glob '!*test*'` → only `main.rs` +
the definition. Providers can be added/attached **without** `right up`:

- **Dashboard `/providers`** (the primary, bot-first control plane):
  `crates/right/src/internal_api_providers.rs:~788` and `~1206` call
  `create_provider` + `attach_to_sandbox` directly. **Does not** call
  `reconcile_for_sandbox` and **does not** ensure v2.
- **config-watcher hot-reconcile** (`sandbox.providers` edited in `agent.yaml`):
  `crates/bot/src/sandbox_supervisor.rs:347` (bring-up) and `:447`
  (`hot_reconcile_providers`) call `reconcile_for_sandbox`. **Neither** ensures v2.

So on a gateway whose flag is off (fresh install, gateway reinstall/recovery,
host migration, or simply an agent created after a gateway that never saw a
`right up` with this code), adding a provider via the dashboard or editing
`agent.yaml` **silently fails to substitute** — identical symptom, outside
`right up`. Since `/providers` is the normal way users add providers, this is
arguably the *more common* real path than `right up`.

**This gap is itself an instance of the bug that bit us**: a single,
easy-to-miss call site. The proto refresh removed the one `cmd_up` call and
nothing caught it.

### Design options (decide in session)

Funnel facts: `reconcile_for_sandbox(client, sandbox, agent_prefix, declared)`
(`providers.rs:~459`) is the supervisor's single attach/detach funnel, but the
dashboard bypasses it with a direct `attach_to_sandbox`. So no single existing
function covers all live attaches.

- **Option A — guard at each entry point.** Call `ensure_v2_enabled` at the top
  of `reconcile_for_sandbox` (covers both supervisor paths) **and** in the
  dashboard provider-create/attach handler(s) in `internal_api_providers.rs`.
  Explicit, no per-attach overhead. Risk: future 4th entry point forgets it —
  the exact drift that caused this bug.
- **Option B — structural invariant at the primitive.** Call `ensure_v2_enabled`
  inside `attach_to_sandbox` (the lowest-level attach), so you *cannot* attach
  without ensuring the flag. Most robust against future drift. Cost: N
  redundant idempotent upserts per reconcile loop; and a layering smell (a
  narrow "attach" primitive mutating global gateway config). Mitigate with a
  process-level memo (`OnceCell`/`AtomicBool` "already ensured this gateway this
  process") so it fires ~once.
- **Option C — memoized gateway-prep step.** A `ensure_v2_enabled` wrapped in a
  per-process memo, called at the two real funnels (reconcile + dashboard add).
  Combines A's explicitness with B's cheapness. **Recommended.**

### Error semantics per path (don't blindly copy `cmd_up`)
- **Dashboard add:** the user is explicitly adding a provider → if
  `ensure_v2_enabled` fails, it's a hard, surfaced error (the add cannot work).
- **hot-reconcile:** consistent with reconcile's converge-on-retry model —
  fatal/log when a provider is declared, retry next tick; tolerate when none
  declared.
- **`cmd_up`:** already fatal-if-any-provider / warn-otherwise. Keep.

### Test plan for P2
- Unit: memo fires once; each entry point invokes ensure-v2 before attach
  (mock `update_config` assertion). 
- Integration (already covered for the test path): the live provider tests
  enable v2; add/confirm a dashboard-path test if feasible.
- Re-validate end-to-end on the Linux gateway container (recipe below) with the
  flag reset to false: a dashboard-style add must now self-enable + substitute.

---

## P3 — fold-vs-compose (architecture discussion, no rush)

**Current decision (load-bearing):** Right's generated `policy.yaml` is
deliberately provider-free; we rely on the gateway composing attached-provider
profile endpoints. See `docs/architecture/providers.md` and ARCHITECTURE.md
("Provider endpoints are OpenShell profile composition, never Right-folded
`policy.yaml` stanzas").

**Why it was chosen:** ordering. In permissive mode the hostless `tls: skip`
catch-all (ports 443/80, broad `allowed_ips`) would, if it appeared before the
provider L7 endpoints, IP-match and raw-tunnel every provider host — stranding
the placeholder (no termination ⇒ no substitution). Gateway composition gets
the ordering right; a hand-folded `policy.yaml` must replicate it.

**What the finding changes:** the decision stands (composition *works* on Linux
once the flag is on), but its correctness now visibly hinges on a chain that is
**default-off and fails silently**: (a) `providers_v2_enabled` (default false,
persists invisibly in gateway state); (b) composition only at `GetSandboxConfig`
pull time; (c) `AttachSandboxProvider` doesn't recompose — we lean on
`policy set --wait` as the refetch barrier (which no-ops on unchanged hash).
Each link can break with no error; only the outbound CONNECT is denied.

**The trade-off to weigh:** folding endpoints into our `policy.yaml`
(with correct ordering — provider L7 endpoints before the hostless catch-all)
removes the dependency on (a), (b), and (c) entirely; the positive control
proved it works on Linux. But it re-introduces the ordering hazard we avoided
and duplicates logic OpenShell owns. **Recommendation:** keep composition;
document folding-with-correct-ordering as the **fallback** if alpha-OpenShell
keeps regressing this. Don't switch now.

---

## P4 — defense-in-depth (discussion)

"Flag set" and "composition actually happened" diverged once and failed
silently. Cheap insurance, in rough order of value:
- **Read-back after set.** `ensure_v2_enabled` is currently a fire-and-forget
  upsert. Read the setting back (or check `UpdateConfigResponse`) and warn/fail
  if it isn't true.
- **Composition smoke at startup.** Optionally assert that an attached
  provider's endpoint actually appears in the composed policy for one sandbox
  (or that a known provider CONNECT succeeds) — catches future gateway-behavior
  drift, not just the flag.
- **Preflight tie-in.** `right_openshell::preflight` already gates
  `MIN_OPENSHELL_VERSION`; consider asserting the providers-v2 capability there
  so a gateway that can't support it fails loudly at startup, not at first
  substitution.

---

## Prod remediation (open action)

Existing **Linux** production gateways with active providers are still broken
until they pick up this fix. The flag is **gateway-global**, so per host either:
- run the new `right up` (sets it once for all agents on that gateway), or
- one-time `openshell settings set --global --key providers_v2_enabled --value true`.
Decide rollout. (macOS dev hosts already have it true.)

---

## Re-validation recipe (Linux gateway, reproduces CI on macOS)

The macOS Homebrew gateway already uses the docker driver; a second one won't
differ. To get the *Linux* gateway behavior, run the official Linux container:

- `ghcr.io/nvidia/openshell/gateway:0.0.56`, `--platform linux/amd64`, DooD via
  `-v /var/run/docker.sock:/var/run/docker.sock`, port **8080** (≠ the live
  17670 gateway — never touch that or `openshell-right-*` containers), isolated
  bind-mount state at an **identical host+container path** (named volumes break
  the supervisor sideload), `OPENSHELL_DRIVERS=docker`, JWT Ed25519 +
  `allow_unauthenticated_users`, `disable_tls=true`.
- Right's harness uses mTLS; to drive the plaintext repro gateway, add a
  throwaway `OPENSHELL_INSECURE_PLAINTEXT=1` branch in
  `right_openshell::openshell::connect_grpc` (plaintext h2 channel) and revert
  after.
- Read the real OPA deny reason from the **sandbox supervisor** container logs
  (`docker logs <sandbox-container>`), e.g.
  `DENIED /usr/bin/curl -> host:443 [reason:endpoint not allowed by any policy]`
  — the gateway journal alone won't show it.
- A repro gateway container `openshell-repro-gw` (:8080, state at
  `/tmp/openshell-repro-data`) was **left running** from this session. Either
  reuse it for P2 validation or `docker rm -f openshell-repro-gw` to clean up.

---

## Source references

OpenShell v0.0.56 (read directly when discussing):
- `crates/openshell-server/src/grpc/policy.rs` — composition call sites, gated
  by `PROVIDERS_V2_ENABLED_KEY`, at `GetSandboxConfig`.
- `crates/openshell-policy/src/compose.rs` — `compose_effective_policy`,
  `ProviderPolicyLayer` (clones rule incl. binaries intact).
- `crates/openshell-sandbox/{data/sandbox-policy.rego,src/opa.rs,src/proxy.rs,src/procfs.rs}`
  — CONNECT gate, OPA input, `policy_denied`, Linux-only procfs identity.

Right:
- `crates/right-openshell/src/providers.rs` — `ensure_v2_enabled`,
  `reconcile_for_sandbox`, `attach_to_sandbox`, `create_provider`.
- `crates/right/src/main.rs` `cmd_up` — the one current `ensure_v2_enabled` call.
- `crates/right/src/internal_api_providers.rs` — dashboard provider add (~788,
  ~1206); **P2 target**.
- `crates/bot/src/sandbox_supervisor.rs` — reconcile (347, 447) + policy reload
  (369, 473); **P2 target**.
- `docs/architecture/providers.md` — provider subsystem narrative (now documents
  the v2 gate).
- Memory: `project_openshell_gateway_state_masks_regressions`.
