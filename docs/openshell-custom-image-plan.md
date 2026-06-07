# Plan: Ship a Custom OpenShell Sandbox Image for Right Agent

## 1. Current state

We create sandboxes by shelling out to the **CLI**, never gRPC, and never with an image selector:

- `right_openshell::openshell::spawn_sandbox(name, policy_path, upload_dir, providers)` runs `openshell sandbox create --name <n> --policy <p> --no-tty [--upload <dir>] [--provider <p>]*` (`crates/right-openshell/src/openshell.rs:562-594`, confirmed by direct read). No `--from`/`--image`. We always get the **gateway default image**.
- The gRPC `CreateSandbox`/`SandboxSpec`/`SandboxTemplate.image` path is referenced **only** in the test mock (`crates/right-openshell/src/test_mock_server.rs:242-244`). Production uses gRPC only for get/delete/readiness/exec/policy/providers (`openshell.rs:287,437,1657,1742`). So "create is CLI" is a real, undocumented exception to the "gRPC for everything except file transfer + policy set" rule.
- `SandboxConfig` (confirmed `crates/right-agent-config/src/lib.rs:313-330`) has `mode`, `policy_file`, `name`, `providers` — **no image field**, and `#[serde(deny_unknown_fields)]`, so any new YAML key is a hard parse error until we add it.
- The only version gate is the OpenShell CLI/gateway `>= 0.0.50` (`preflight.rs:16`); nothing checks the image, `claude`, or `node`.

**What we assume baked into the image** (not uploaded): `claude` on PATH (invoked by bare name, `cc/invocation.rs:496`), bash login shell (`sync.rs:461`, `sandbox_env.rs:19`), POSIX sh + GNU coreutils/sed (`learning_prefilter.rs:330`, `prompt.rs:104-166`), GNU tar + ssh server (`openshell.rs:1119-1175`), `getent` + `host.openshell.internal` resolution (`openshell.rs:1832-1860`), a `sandbox` user/group with `/sandbox` home (`policy.rs:304-306`), npm/node (`sandbox_env.rs:25-37`), and the OpenShell-injected CA bundle + `HTTPS_PROXY` (runtime-provided, not by us). `binaries: path: "**"` (`policy.rs:317`) means OpenShell runs whatever the image ships — we install nothing.

**What we upload/stage**: minimal staging dir (`.claude/settings.json`, `reply-schema.json`, `.claude.json`, `mcp.json`, `TOOLS.md`); post-READY we sync skills + content-addressed config to `/sandbox/.platform/` and write `env.sh`/`.bashrc`/identity (`docs/architecture/sandbox.md:70-87`, `sync.rs:14-32`).

## 2. Can we substitute our own image, or must we commit upstream?

**No upstream change required. High confidence.** Three non-upstream mechanisms, in order of how I'd adopt them:

1. **CLI `--from <ref>` per-sandbox** — the create command already accepts a full registry ref, a community name, or a local Dockerfile/dir (`openshell sandbox create --help`, verified in the "image plumbing" report: *"a full container image reference (e.g. myregistry.com/img:tag)"*; corroborated by upstream docs in "image mechanics"). This is the CLI analogue of `SandboxTemplate.image`. **This is our path** — it threads one arg into the existing chokepoint.
2. **Gateway TOML `default_image`** — fleet-wide override (`docs.nvidia.com/openshell/latest/reference/gateway-config`). Operator-owned, not per-agent; not in our codegen surface.
3. **`OPENSHELL_COMMUNITY_REGISTRY`** env — redirects bare-name resolution to an internal mirror.

We do **not** need the gRPC `CreateSandbox` route. It would only be forced if we needed `SandboxTemplate` knobs the CLI lacks (`runtime_class_name`, `user_namespaces`, `volume_claim_templates`) — we need none of those for an image swap. Migrating create to gRPC is a separate, larger effort and explicitly out of scope here.

**Contradiction to resolve first** (flagged in the report): `ARCHITECTURE.md:610` lists `sandbox create` under gRPC, but `ARCHITECTURE.md:617` and the code (`openshell.rs:569`) make create a CLI operation. Fix line 610 to remove "create" and document create as a sanctioned CLI exception **in the same PR** that adds image support, so we're not building on a doc that contradicts the implementation.

**Verification TODOs before build** (cheap, do them in a scratch sandbox):
- Confirm `--from <full-registry-ref>` is honored by our pinned **v0.0.50** specifically (docs reflect v0.0.54; "custom-image paths" report could not pin behavior to 0.0.50). Run `openshell sandbox create --from <our-ref> --name probe-img ...` and verify it boots.
- Confirm the gateway default image identity. Reports contradict: gateway docs example shows `ghcr.io/nvidia/openshell/sandbox:latest`, but the CLI resolves community `base` to `ghcr.io/nvidia/openshell-community/sandboxes/base:latest`. Read an actual `gateway.toml` or `openshell sandbox get` on a default sandbox to learn which image our deployments actually boot — that's the **FROM** we must inherit.

## 3. Image contract (what any custom image MUST satisfy)

From the BYOC example + compute-drivers reference ("image mechanics" / "custom-image paths" reports), plus our own baked-in assumptions (§1):

**OpenShell hard requirements:**
- **Real Linux base** — distroless and `FROM scratch` are unsupported. Standard glibc Ubuntu/Debian is safe (supervisor binary arch/libc assumptions for musl/Alpine are unverified — see §7).
- **`sandbox` user with UID/GID `1000660000`** (high UID to avoid host collisions), owning a **writable `/sandbox`** workdir (`sandbox:sandbox`).
- **`iproute2` installed** for network-namespace isolation.
- **No baked-in agent/socket; no fixed CMD/ENTRYPOINT** — the supervisor is injected at runtime and replaces CMD/ENTRYPOINT; the real start command is passed via `-- <cmd>` (we pass nothing today, relying on the default image's behavior — verify our create call still boots correctly when `--from` points at a custom image without a `--` command).
- No image-baked seccomp/landlock/runtime-class requirement (medium confidence; isolation is supervisor/driver-enforced).

**Right Agent additional requirements** (must carry forward or we break at runtime — these are the break points if the base changes):
- `claude` on PATH (bare-name invocation), bash login shell, POSIX sh + GNU coreutils + **GNU sed** + **GNU tar**, `getent`, ssh server, `node`/`npm`, and `host.openshell.internal` resolvable. CA bundle + `HTTPS_PROXY` are OpenShell-injected at runtime, so inheriting from the OpenShell `base` image (rather than a bare Ubuntu) preserves them for free.

The safe way to satisfy **both** sets at once: **`FROM` the OpenShell `base` image** (which already ships claude/node/git/gh and the `sandbox` user) and layer on top, rather than rebuilding the contract from raw Ubuntu.

## 4. Recommended approach

**(b) Fork/extend the default image — `FROM ghcr.io/nvidia/openshell-community/sandboxes/base:<pinned-digest>`, selected per-agent via the CLI `--from` override.**

Rationale against the hard rules:
- vs **(a) plain BYOC of a from-scratch image**: extending `base` inherits the entire image contract (sandbox user, claude, node, CA/proxy wiring) so we don't re-derive it and drift from upstream. Cheaper and safer.
- vs **(c) upstream PR**: unnecessary — no upstream change is needed, and a PR puts our release cadence behind NVIDIA's. Keep (c) in reserve only if we later need a `SandboxTemplate` knob the CLI lacks, or an allowed-images gateway policy (the report could not confirm one exists).
- Pin by **digest**, not `:latest`, so the base is reproducible and we control the `claude`/`node` versions we ship — this is also where we'd add the missing image/claude version gate.

**Handling the "no sandbox recreation as a migration path" hard rule — this is the load-bearing constraint.** A base-image change is inherently **`Regenerated(SandboxRecreate)`-class** (same class as a filesystem/landlock policy change, `ARCHITECTURE.md`): the image is fixed at sandbox creation; you cannot hot-swap it into a running sandbox. We therefore **cannot** silently flip every deployed agent to a new image. Make it safe and rule-compliant by:

1. **Opt-in only.** No `image:` in `agent.yaml` ⇒ behavior is byte-identical to today (gateway default image, no `--from`). Backward-compatible default = previous behavior, satisfying "upgrade-friendly design." Already-deployed agents keep running untouched.
2. **Adoption goes through the existing `SandboxRecreate` migration flow** (`docs/architecture/lifecycle.md` → "Sandbox migration (filesystem policy change)"), which already does **`right agent backup` → recreate → restore** — never a bare delete, never `right agent init`. Setting/changing `image:` is classified `SandboxRecreate` and reuses that backup/restore path verbatim. This is the *only* way it touches a live agent, and it preserves agent data per "Never delete sandboxes for recovery."
3. **No CLI-driven mass migration.** An operator changes one agent's `image:`, the config watcher classifies it `SandboxRecreate`, and that single agent migrates via backup/restore. Fleet rollout is N independent opt-in migrations, each reversible by reverting the field.

## 5. Build pipeline

- **Base**: `FROM ghcr.io/nvidia/openshell-community/sandboxes/base@sha256:<digest>` (resolve the exact ref our gateway actually defaults to first — §2 TODO). Pull the upstream Dockerfile (`raw.githubusercontent.com/NVIDIA/OpenShell-Community/main/sandboxes/base/Dockerfile`) as the reference for what's already present so we don't double-install.
- **Layers we add**: only what `base` lacks for Right Agent (likely little — base already has claude/node/git/gh). Candidates: pin a specific `claude` version if base's is too new/old for us, any extra coreutils/whisper-adjacent host tools (note: ffmpeg/whisper run on the **host**, not the sandbox — `right-stt`; do **not** add them to the image).
- **Preserve the contract**: do not change the `sandbox` user UID/GID `1000660000`, keep `/sandbox` writable+owned, keep `iproute2`, do not set a CMD/ENTRYPOINT (supervisor replaces it).
- **Registry**: push to a Right-controlled private registry. Pin by digest in the `image:` field. For **remote gateways**, the ref must be an already-pushed registry image (local Dockerfile build needs a LOCAL gateway). 
- **Auth**: private-registry pull is documented **only** for the Kubernetes driver (`image_pull_secrets = ["regcred"]` + `image_pull_policy`); Docker/Podman registry auth is unverified (§7). Determine our deployment driver before committing to a private registry — if Docker/Podman, we may need host-level `docker login`/cred helpers, which is unconfirmed.
- **Version pinning / gate**: introduce a `MIN`-style constant (mirroring `MIN_OPENSHELL_VERSION`) or at minimum record the pinned digest in a vendored manifest (cf. `proto/UPSTREAM.md` for the proto pin) so the image is auditable and reproducible. Build via CI on the same cadence we bump the OpenShell pin.

## 6. Code changes needed

Concrete, minimal slice (CLI `--from` path; no gRPC migration):

1. **`crates/right-agent-config/src/lib.rs:313` — add the config field.** Add `#[serde(default, skip_serializing_if = "Option::is_none")] pub image: Option<String>` to `SandboxConfig`. Because of `deny_unknown_fields`, this is required before any YAML can carry it. Update the `Default` impl (`:332`) to `image: None` (preserves today's behavior). Decide value semantics: full registry ref vs community name (we want full ref for reproducibility).

2. **`crates/right-openshell/src/openshell.rs:562` — thread it into the chokepoint.** Add an `image: Option<&str>` param to `spawn_sandbox`; when `Some`, `cmd.arg("--from").arg(image)` before `--no-tty`. Update the single production caller at `openshell.rs:1620` (`spawn_sandbox(sandbox, policy_path, staging_dir, &[])`) to pass the resolved image through. Keep the `&[]`/`None` default so `mode: none` and image-less agents are unchanged.

3. **Codegen category + watcher classification.** Image selection is **not** a codegen *file* output, so it doesn't get a `CodegenFile` registry entry. Instead it's a sandbox-creation input classified **`SandboxRecreate`**:
   - **`crates/bot/src/config_watcher.rs::diff_classify`** — add `image` to the two-stage smart-diff. It is **NOT** hot-reloadable (unlike `model`/`debug`/`sandbox.providers`). A change to `sandbox.image` must classify as a recreate-triggering change that routes through the backup/recreate/restore migration (same machinery as a filesystem-policy change), **not** a graceful restart and **not** `ProvidersReload`. This is the enforcement point for the §4 safety rule.

4. **Wire the resolved image to the create caller** — the ensure-sandbox flow (around `openshell.rs:1620`, called from the bot's sandbox supervisor) must read `agent.yaml::sandbox.image` and pass it down. Verify it's read at both initial create and the recreate/migration path so opt-in adoption actually re-creates with the new image.

5. **Docs (mandatory, same PR):**
   - Fix the `ARCHITECTURE.md:610` vs `:617` contradiction — remove `create` from the gRPC list, document CLI create as a sanctioned exception alongside file transfer and `policy set --wait`.
   - Add `image` to the **Configuration Hierarchy** table and note its `Regenerated(SandboxRecreate)` category in the **Upgrade & Migration Model**.
   - Update `docs/architecture/sandbox.md` (image assumptions / what's baked vs uploaded) per the cite-on-touch rule.
   - If a CLI flag is added to set `image` via `right agent config`, expose it (the "agent config must expose all user-facing settings" rule) — unless we deliberately make it bot/dashboard-managed, in which case document the exception.

6. **Tests:** image-less config → no `--from` (byte-identical args to today); `image: Some(ref)` → `--from <ref>` present. `config_watcher` test asserting `sandbox.image` change classifies `SandboxRecreate`, not hot-reload. A live `ci_openshell_`-prefixed `#[ignore = "ci-openshell: ..."]` test that creates a sandbox from our custom ref and asserts it reaches READY + `claude` resolves (per the live-OpenShell CI convention).

## 7. Open questions / verification TODOs

- **v0.0.50 parity.** All upstream findings reflect "latest" (v0.0.54, released 2026-06-02). Confirm `--from <full-ref>`, `default_image`, `OPENSHELL_COMMUNITY_REGISTRY`, and the UID `1000660000` contract are identical at our pinned **v0.0.50** — or bump the pin. Verify by reading the v0.0.50-tagged docs/source and a probe `create`.
- **Which image is the real default?** Reports contradict: `ghcr.io/nvidia/openshell/sandbox:latest` (gateway-config example) vs `ghcr.io/nvidia/openshell-community/sandboxes/base:latest` (CLI community resolution). Resolve by inspecting an actual `gateway.toml`/`openshell sandbox get` before choosing our `FROM`.
- **Non-k8s private-registry auth.** `image_pull_secrets` is documented Kubernetes-only. Docker/Podman/MicroVM private-registry auth is unverified — determine our driver and its auth mechanism before standing up a private registry.
- **Supervisor libc/arch constraints.** Whether the injected supervisor binary imposes glibc/arch expectations beyond "standard Linux base" (e.g. musl/Alpine incompatibility) is undocumented. Sticking to `FROM` the OpenShell `base` (glibc Ubuntu Noble) sidesteps this.
- **Allowed-images gateway policy.** Whether a gateway can allow-list which `--from` refs are accepted is unconfirmed (no such gate found). If it exists, our private ref must be allow-listed; if not, no action.
- **CA bundle + `HTTPS_PROXY` source.** Asserted (medium confidence) to be OpenShell runtime-injected, not image-baked. `FROM base` makes this moot, but confirm if we ever consider a non-base FROM.
- **`-- <command>` requirement.** BYOC docs say the supervisor replaces CMD/ENTRYPOINT and you pass the start command after `--`; our `spawn_sandbox` passes none today. Verify a custom-image create still boots `claude`-capable without an explicit `--` command (likely fine when inheriting `base`, but must be probed).

---

## 8. Automatic sandbox upgrade at `right up` (data-preserving)

This section extends §4–§6 to satisfy the explicit user requirement: when `right up` runs against agents whose sandboxes boot a stale base image, it must announce "upgrade needed", back up automatically, then recreate each sandbox onto the new image **without data loss**. It reuses the existing migration machinery verbatim — no new delete-and-recreate path, no `right agent init`.

### 8.1 Detection oracle — how we know a sandbox is stale

Two candidate oracles were both validated against the live v0.0.50 gateway:

- **(a) gRPC `GetSandbox` image field.** *Confirmed exposed.* The Detection-Oracle experiment read `Sandbox.spec.template.image` live off the real READY sandbox `right-him-20260516-1649` and got `ghcr.io/nvidia/openshell-community/sandboxes/base:latest`. The proto carries it at `crates/right-openshell/proto/openshell/openshell.proto` (`message Sandbox` L294 → `SandboxSpec spec = 2` L298 → `SandboxTemplate template = 6` L314 → `string image = 1` L328–330), returned by `GetSandbox` (L28) inside `SandboxResponse` (L475–477). The code already has the full `Sandbox` value in hand in `get_sandbox_readiness` (`crates/right-openshell/src/openshell.rs:436–454`) and `resolve_sandbox_id` (`:1652–1677`) — neither reads `spec` today, so surfacing the image is a few lines, no new RPC.
- **(b) recorded created-from ref vs binary-baked desired pin.** A `recorded_image: Option<String>` written next to `SandboxConfig.name` (`crates/right-agent-config/src/lib.rs:326`) at create time, compared against a `DESIRED_SANDBOX_IMAGE` const baked into the binary (mirroring `MIN_OPENSHELL_VERSION`, `crates/right-openshell/src/preflight.rs:16`).

The CLI is a confirmed dead end for reads: `openshell sandbox get` has no image field and no `-o json` (only `--policy-only`); structured reads exist only via `sandbox list -o json`, whose schema is unconfirmed (CLI-introspection experiment, high confidence). So if we read the live image, it MUST be over gRPC.

**Chosen design: (b) as the trigger, (a) as a corroborating/self-healing read.** The reason is the experiment's load-bearing caveat: `GetSandbox` returns a **mutable `:latest` tag**, not a content digest, and the proto exposes no resolved-digest field anywhere (`SandboxStatus`/`SandboxCondition` lack it; annotations came back empty). Two sandboxes both reporting `:latest` can run different underlying layers — so naive `spec.template.image == DESIRED` string-equality cannot detect drift while we ship `:latest`. The only deterministic signal we control is **a pinned-by-digest desired ref** (per v1 §4/§5: pin `base@sha256:<digest>`, never `:latest`) compared against the **`recorded_image` we wrote when we created that sandbox**. Drift = `recorded_image != DESIRED_SANDBOX_IMAGE`. This is a pure in-binary compare — same shape as `filesystem_policy_changed` (`openshell.rs:1976`) — needs no live gateway round-trip, and works for offline/unreachable gateways.

`GetSandbox.spec.template.image` is the **fallback / reconciliation read**: on startup we can additionally read the live image to detect a sandbox whose `recorded_image` is missing (pre-upgrade agents created before this field existed) or disagrees with reality (manual gateway-side change). A missing `recorded_image` is treated as "unknown → assume stale" so already-deployed agents adopt the pin on their next `right up`. Per the "debuggable signals" rule we prefer the gRPC live read over trusting `recorded_image` blindly when the two disagree, and we log both values.

### 8.2 The `right up` UX

`right up` runs only one shared gateway preflight + cross-agent codegen + process-compose; it has **zero per-agent sandbox calls** today, and per-agent sandbox `ensure` happens in the **bot** (`sandbox_supervisor::bring_up_sandbox`), not in `right up` (Migration-reuse finding, high confidence; discovery loop at `crates/right/src/main.rs:2694–2696`). To surface "upgrade needed" at `right up` we add a **per-agent image-drift inspect loop** after agent discovery (near `main.rs:2696`), mirroring the structure of `maybe_migrate_sandbox` (`main.rs:6070`) but keyed on image drift, not filesystem-policy drift.

Per stale agent, before the destructive step, print via `right_ui` (never raw `println!`/`inquire` — `maybe_migrate_sandbox` currently uses both and must NOT be copied as-is). Use the `Recap`/`warn` surface already used at `main.rs:1861`/`:2356` and the `Glyph::Warn` pattern at `main.rs:1532–1536`:

```
⚠ <agent>: sandbox '<name>' runs <recorded_image>; upgrade required to <DESIRED_SANDBOX_IMAGE>.
  Backing up and recreating — agent data is preserved. Old sandbox kept until restore verifies.
```

**Reconciling "automatic" with the safety rules.** The user said: print "upgrade needed", make backups, then upgrade — automatically. The platform rule says recreation is destructive and must never be silent. We reconcile by making **detection and backup fully automatic and non-interactive**, and the **recreate explicit and observable** rather than gated behind a y/n prompt (`right up` is often non-interactive, so `inquire::Confirm` is wrong here):

- Default behavior at `right up`: detect drift → emit the warn line → run the backup → recreate → restore. The destructive recreate is announced with per-step progress (the migration fn already prints "Step N/6"), and the old sandbox is provably retained until restore verifies (§8.4), so "automatic" never means "data-risking".
- An explicit escape hatch `right up --no-sandbox-upgrade` (and the inverse default) lets an operator defer. Drift that is deferred re-warns on every `right up` until adopted. This keeps the operator in control without making the common path interactive.

This is more aggressive than the filesystem-policy path (which today fails startup and tells the user to run `right agent config` — `sandbox_supervisor.rs:190–202`), and that is intentional: the user explicitly asked for auto-upgrade. We do **not** weaken the filesystem-policy fail-closed behavior; image upgrade gets its own auto path because backup+restore makes it safe.

### 8.3 The upgrade flow (reuses `perform_migration`)

The upgrade is exactly the existing migration, with one delta: the new sandbox is created **from the pinned image**. Reuse `perform_migration(home, agent_name, old_sandbox, mtls_dir)` (`crates/right/src/main.rs:6170`). The live rehearsal proved the whole shape works at v0.0.50 (see §8.7). Step map:

| Step | Code | Image-upgrade delta |
|---|---|---|
| 1 Backup old sandbox only | `main.rs:6181–6219` — `ssh_tar_download` to `backups_dir/<ts>/sandbox.tar.gz` | none |
| 2 Create new from **new image** | `main.rs:6243–6244` `spawn_sandbox(&new_sandbox, &policy_path, Some(&staging), &[])` | thread `image: Option<&str>` into `spawn_sandbox` (`openshell.rs:562`); when `Some`, add `--from <ref>` (v1 §6.2). Proven: E3 created `--from <full ref>` at v0.0.50, sandbox reached Ready |
| 2b Wait READY | `main.rs:6248–6265` `wait_for_ready` (`openshell.rs:344`) | none. E3 note: `create` exit-0 is at compute-request, not READY — readiness MUST be polled (already is) |
| 3 Resolve host IPs + apply exact Right MCP policy | `main.rs:6267–6286` `resolve_sandbox_id`/`wait_for_ssh`/`apply_exact_right_mcp_policy_for_sandbox` | none — policy is image-independent |
| 4 Generate SSH config | `main.rs:6288–6296` `generate_ssh_config` | none |
| 5 Restore via tar + verify | `main.rs:6298–6317` `ssh_tar_upload` | E3 verified marker survived restore byte-identical; restore strips `strip-components 1`, excludes cache/venv/npm/uv — download/upload pair must stay matched (`openshell.rs:1119–1175`) |
| 6 Write `sandbox.name` + **`recorded_image`**, then delete old | `main.rs:6319–6351` `update_agent_yaml_sandbox_name` + `delete_sandbox` | **add**: write `recorded_image = DESIRED_SANDBOX_IMAGE` via `write_merged_rmw` in the same `agent.yaml` update so idempotency holds (§8.4). Old delete stays last |

Restore verification (§8.4) gates step 6's delete. Because the field semantics and create-path are the only deltas, the migration body is shared; pass the resolved image through to `spawn_sandbox`.

### 8.4 Failure safety & idempotency

- **Old sandbox retained until restore verified.** Confirmed in code: on `ssh_tar_upload` failure, `perform_migration` deletes the **new** sandbox, removes the new SSH config, returns `Err`, and **preserves the old** (`main.rs:6301–6316`). The old `delete_sandbox` sits strictly **after** the restore-success print and the `agent.yaml` update (`:6317`→`:6321`→`:6339`) and is best-effort/non-failing (`delete_sandbox` returns nothing, `wait_for_deleted` discarded — `openshell.rs:1425`, `:299`). This already satisfies "old sandbox deleted ONLY after restore verified." Add an explicit post-restore marker/size assertion before step 6 so verification is a positive signal, not merely "upload didn't error" (E3 verified a marker file round-trips identically — reuse that technique as the check).
- **Idempotent re-run.** After a successful upgrade, `recorded_image == DESIRED_SANDBOX_IMAGE`, so the §8.1 compare returns no-drift and the §8.2 loop skips the agent entirely. Re-running `right up` does nothing — same property as the policy reuse branch (`sandbox_supervisor.rs:203–211`). The `recorded_image` write goes through `write_merged_rmw` (`agent.yaml` is `MergedRMW`), preserving unknown fields.
- **Partial-failure recovery.** If create/READY/restore fails, the new sandbox is rolled back, the old is intact, `recorded_image` is unchanged (still the old ref), and the error propagates (no swallow). The next `right up` re-detects drift and retries from a clean state. Backup tars are immutable per-timestamp, so a retry never overwrites a prior recovery point.
- **Backups land at** `~/.right/backups/<agent>/<YYYYMMDD-HHMM>/sandbox.tar.gz` via `right_config::backups_dir` (`right-config/src/lib.rs:182`, used at `main.rs:6197–6204`).

### 8.5 Where it runs

Detection and the warn line must appear at **`right up` (CLI)** per the user requirement, and `right up` is the right host because **it is the only entrypoint that is already non-bot and already calls `perform_migration`'s sibling `maybe_migrate_sandbox`** — both live in the `right` crate with `mtls_dir`, gRPC, and migration in scope. The bot's `sandbox_supervisor::bring_up_sandbox` (`crates/bot/src/sandbox_supervisor.rs:77`) is where per-agent ensure normally runs, but it deliberately **fails closed** on drift rather than auto-recreating, and is the wrong place for an interactive/observable backup+recreate that the user wants to see at `up` time.

Resolution: add the image-drift inspect+upgrade loop to `cmd_up` (after discovery, ~`main.rs:2696`), reusing `perform_migration`. Do **not** duplicate detection into the bot — instead, the bot's startup should *also* read `spec.template.image` (cheap, already-in-hand) and **log** drift for observability, but never auto-recreate (recreate stays a `right up` operation, consistent with "no CLI-driven mass migration" — each agent migrates independently). This avoids a second control plane racing the first.

### 8.6 Code changes (extends v1 §6)

1. **Desired-image pin constant.** Add `pub const DESIRED_SANDBOX_IMAGE: &str = "ghcr.io/nvidia/openshell-community/sandboxes/base@sha256:<digest>";` in `crates/right-openshell/src/preflight.rs` (next to `MIN_OPENSHELL_VERSION:16`) or a sibling `image.rs`. Pin by digest, not `:latest` — required for the §8.1 compare to be meaningful. Record the pin in a vendored manifest like `proto/UPSTREAM.md`.
2. **Config fields.** In `crates/right-agent-config/src/lib.rs:313` `SandboxConfig` (note `deny_unknown_fields:312`), add per v1 §6.1 `image: Option<String>` (operator override of the pin) **and** `recorded_image: Option<String>` (what the current sandbox was created from; written by codegen/migration, not the user). Both `#[serde(default)]`; update the `Default` impl (`:332`) to `None`. The desired ref = `image` override if set, else `DESIRED_SANDBOX_IMAGE`.
3. **`spawn_sandbox` plumbing.** Add `image: Option<&str>` (v1 §6.2, `openshell.rs:562`); when `Some`, `--from <ref>` before `--no-tty`. Update both callers: `perform_migration` (`main.rs:6244`) and the bot ensure path (`openshell.rs:1620`).
4. **`config_watcher` classification.** `sandbox.image` and `sandbox.recorded_image` changes are **`RestartRequired`/recreate**, never hot-reload — they must stay out of `normalize_for_reload_diff` (`crates/bot/src/config_watcher.rs:107–119`) so they fall through to the `RestartRequired` default (`:104`). Not `ProvidersReload`. (Pin+config finding, high confidence.)
5. **`cmd_up` inspect+upgrade loop.** New per-agent function near `main.rs:2696` mirroring `maybe_migrate_sandbox` but keyed on `recorded_image != desired`; calls `perform_migration` with the resolved image threaded through. Add `--no-sandbox-upgrade` flag (§8.2).
6. **`recorded_image` write in `perform_migration` step 6** via `write_merged_rmw`, set to the resolved desired ref (§8.3).
7. **`right_ui` output.** Replace this path's `println!`/`eprintln!`/`inquire` with `right_ui` `Recap`/`Glyph::Warn` (per "Brand-conformant CLI output"; pattern at `main.rs:1532–1536`, `:1861`, `:2356`). Non-interactive — no `inquire::Confirm`.
8. **Docs (same PR):** Configuration Hierarchy + Upgrade & Migration Model tables gain `image`/`recorded_image` as `Regenerated(SandboxRecreate)`-class inputs; `docs/architecture/lifecycle.md` "Sandbox migration" gains the image-upgrade trigger; fix the `ARCHITECTURE.md` gRPC-vs-CLI `create` contradiction (v1 §6.5).

### 8.7 Verification / experiments still owed

What the live rehearsal **did** prove (so these are settled, not owed):
- **Marker survived restore byte-identical.** `UPGRADE_MARKER_v1_a4ae46-rehearsal` was written to sandbox A, absent in B pre-restore, identical after `tar xzpf` restore (exit 0) — data preservation across recreate is confirmed at v0.0.50.
- **`--from <full image ref>` works at v0.0.50.** Sandbox `right-imgexp-770002` created `--from` an explicit ref reached Ready. This retires v1 §7's "v0.0.50 parity for `--from`" open question for the full-ref form.
- **`GetSandbox` exposes the live image** (`spec.template.image`), CLI does not (§8.1).

Still owed:
- **Digest-level drift.** GetSandbox returns a tag, not a digest, and no proto field carries the resolved digest. We sidestep this by detecting via `recorded_image` vs a digest-pinned `DESIRED_SANDBOX_IMAGE` — but that only works if our pin is a digest and codegen writes that exact ref to `recorded_image`. If the gateway resolves a digest at create time that we want to verify back, that is an OpenShell proto gap to raise upstream. Owed: a `ci_openshell_`-prefixed test asserting `recorded_image` written at create == the digest pin.
- **Bot orchestration around `--from` recreate not exercised** (rehearsal was manual CLI). Owed: a live `ci_openshell_` test that drives the new `cmd_up` loop end-to-end (drift → backup → recreate-from-pin → restore-verify → `recorded_image` updated → idempotent re-run no-ops).
- **`--from` against an image with a different internal layout than the backup** untested (rehearsal recreated onto `base`, same layout). The restore tar excludes cache/venv/npm/uv and strips one path component; a custom image whose `/sandbox` layout diverges could mis-restore. Owed: probe restore onto our actual custom image.
- **`spawn_sandbox` passes no `-- <command>`.** v1 §7's open question stands; verify a custom-image create still boots `claude`-capable without an explicit `--` (likely fine inheriting `base`).
- **Private-registry pull auth** (v1 §7) unchanged — determine the deployment driver before pinning to a private ref.

### Revisions to earlier sections

- **§7 "v0.0.50 parity" → RESOLVED for `--from`.** The live rehearsal (E3) confirmed `openshell sandbox create --from <full-registry-ref>` is honored at the pinned v0.0.50 and the sandbox reaches READY. The broader §7 item (UID `1000660000`, `default_image`, `OPENSHELL_COMMUNITY_REGISTRY` parity) remains open.
- **§7 "Which image is the real default?" → RESOLVED.** Live `GetSandbox` on a default-created sandbox returns `ghcr.io/nvidia/openshell-community/sandboxes/base:latest` (the CLI community-resolution form), **not** the gateway-docs example `ghcr.io/nvidia/openshell/sandbox:latest`. Our `FROM` inherits from `…/openshell-community/sandboxes/base`.
- **§4 detection assumption added.** v1 §4 did not specify how adoption is *detected*; §8.1 establishes that detection is a binary-baked digest pin vs a recorded `recorded_image`, with gRPC `spec.template.image` as the corroborating read — because GetSandbox only exposes a mutable `:latest` tag, raw GetSandbox string-equality is **insufficient** as the sole oracle.
- **§6.1 config field — extended.** v1 added only `image: Option<String>`; §8.6 adds a second field `recorded_image: Option<String>` (created-from record) required for idempotent auto-detection. Both are `SandboxRecreate`-class and excluded from hot-reload.
- **§3/§5 no change.** Image contract and build pipeline are unaffected; the upgrade mechanism is orthogonal to what the image contains.