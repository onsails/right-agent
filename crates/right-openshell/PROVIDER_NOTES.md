# Provider Probe Notes

Scratch file for Task 1 probe results. **DELETED before Phase 10.**

OpenShell version probed: **0.0.42**
Proto path: `crates/right-openshell/proto/openshell/openshell.proto`
Date: 2026-05-27

---

## Step 1: Provider attach/detach RPC absence — VERIFIED LIVE

```
grep -n "rpc " .../openshell.proto | grep -i "attach|detach|UpdateSandbox"
# → no output
```

**Confirmed:** There are no `AttachProvider`, `DetachProvider`, or `UpdateSandbox` RPCs in the proto.
The `OpenShell` service has: `CreateProvider`, `GetProvider`, `ListProviders`, `UpdateProvider`,
`DeleteProvider`, `GetSandboxProviderEnvironment`. That is all.

**Attach/detach at creation time:** `SandboxSpec.providers` (field 8,
`repeated string`) in `datamodel.proto` is the only in-proto mechanism for provider association —
it is set at `CreateSandbox` time and lists provider names to attach.

**Runtime attach/detach:** `openshell sandbox provider attach <sandbox> <provider>` and
`openshell sandbox provider detach <sandbox> <provider>` exist as CLI commands and work at
runtime, but there is no corresponding gRPC RPC. They must be implemented via CLI shelling out
(`exec_openshell_cli`), not via the gRPC client.

**Design decision locked:** Tasks 5 and 6 must shell out to
`openshell sandbox provider attach/detach`. The gRPC path cannot be used.

**Task 9 note:** `spawn_sandbox` can pass provider names in `SandboxSpec.providers` at creation
time. This is the clean path for initial attachment. Runtime attach (for providers added after
sandbox creation) still requires the CLI.

---

## Step 2: `providers_v2_enabled` gateway setting — VERIFIED LIVE

```
openshell settings get --global
# Scope:         global
# Settings Rev:  1
# Settings:
#   agent_policy_proposals_enabled = <unset>
#   ocsf_json_enabled = <unset>
#   providers_v2_enabled = true
```

**Confirmed:** `providers_v2_enabled` is a real registered setting key at global scope, currently
`true` on the test gateway. The `openshell settings set --global --key providers_v2_enabled --value true --yes`
command works.

**gRPC mapping** (from proto `UpdateConfigRequest`):
- `global = true`
- `setting_key = "providers_v2_enabled"`
- `setting_value = SettingValue { bool_value: true }`
- `name` field empty (not used when `global = true`)

**`GetGatewayConfigRequest`** returns a `GetGatewayConfigResponse` with
`map<string, SettingValue> settings`. Check `settings["providers_v2_enabled"].bool_value` — an
unset key returns an empty `SettingValue` (all fields zero/false), so treat missing as `false`.

**`ensure_v2_enabled` implementation** (Task 4):
1. Call `GetGatewayConfig`.
2. If `settings["providers_v2_enabled"].bool_value == true` → no-op.
3. Otherwise call `UpdateConfig` with the gRPC fields above.
4. If `UpdateConfig` fails AND any agent has providers → fatal error at `right up`.

---

## Step 3: Provider profiles — VERIFIED LIVE

```
openshell provider list-profiles --output json
```

The gateway returns **10** profiles. Right exposes **8** (hides `claude` and `outlook`).

| Slug       | Display name       | Category       | Credential key  | Primary env var injected | Credential env_var name |
|------------|--------------------|----------------|-----------------|--------------------------|-------------------------|
| `anthropic`| Anthropic API      | inference      | `api_key`       | `ANTHROPIC_API_KEY`      | `ANTHROPIC_API_KEY`     |
| `nvidia`   | NVIDIA             | inference      | `api_key`       | `NVIDIA_API_KEY`         | `NVIDIA_API_KEY`        |
| `openai`   | OpenAI             | inference      | `api_key`       | `OPENAI_API_KEY`         | `OPENAI_API_KEY`        |
| `codex`    | Codex              | agent          | `api_key`       | `OPENAI_API_KEY`         | `OPENAI_API_KEY`        |
| `copilot`  | GitHub Copilot     | agent          | `github_token`  | `COPILOT_GITHUB_TOKEN`   | `COPILOT_GITHUB_TOKEN`  |
| `opencode` | OpenCode           | agent          | `api_key`       | `OPENCODE_API_KEY`       | `OPENCODE_API_KEY`      |
| `github`   | GitHub             | source_control | `api_token`     | `GITHUB_TOKEN`           | `GITHUB_TOKEN`          |
| `gitlab`   | GitLab             | source_control | `api_token`     | `GITLAB_TOKEN`           | `GITLAB_TOKEN`          |
| `generic`  | (no profile)       | —              | user-supplied   | user-supplied            | user-supplied           |
| ~~`claude`~~ | ~~Claude Code~~  | ~~agent~~      | hidden by Right | —                        | —                       |
| ~~`outlook`~~ | ~~Outlook~~     | ~~messaging~~  | hidden by Right | —                        | —                       |

**Notes on the table:**
- "Primary env var injected" = the first env var in `credentials[0].env_vars`. Multiple aliases
  exist for some types (e.g. `copilot` also injects `GH_TOKEN` and `GITHUB_TOKEN`;
  `gitlab` also injects `GLAB_TOKEN` and `CI_JOB_TOKEN`). The placeholder the sandbox sees is
  for the primary env var only; aliases are not guaranteed to be populated by the provider proxy.
- `codex` credential name is `api_key` but env var is `OPENAI_API_KEY` — same credential key name
  as `openai` type. Two different provider objects, same env var. If an agent has both, they would
  collide. The 409 EnvVarCollision check in Task 14 needs to account for this.
- `outlook` has zero credentials and zero endpoints — it is an empty placeholder profile.
  Hiding it is the right call.
- The spec lists 9 exposed types. Actual count: 8 built-ins + `generic` = 9. This matches.
  `outlook` adds a 10th that was not in the spec — hiding it is correct.

**Task 8 hardcoding:** Use this table verbatim. The catalog is static — `list-profiles` will be
called at bot startup and cached, but the catalog in `providers.rs` is the fallback for UX
display without a live gateway.

---

## Step 4: Placeholder string format — ASSUMED (sandbox creation failed)

New sandboxes entered Error phase on this dev machine — the k3s/Kubernetes backing was not
available (existing sandboxes were created by `right init`/`right up` and survive). Creating a
throwaway sandbox to verify the placeholder format live was not possible without modifying a live
agent sandbox.

**Assumed format** (from spec `docs/superpowers/specs/2026-05-27-providers-design.md` and
`docs/superpowers/plans/2026-05-27-providers.md`):

```
openshell:resolve:env:v<digits>_<CREDENTIAL_KEY>
```

Example: `openshell:resolve:env:v17329906524197465519_MY_TOKEN`

The `<digits>` part is a numeric version/nonce generated by the gateway per provider credential.
The `<CREDENTIAL_KEY>` is the key passed to `--credential KEY=VALUE` at creation time (for
generic) or the built-in credential key name (e.g. `api_key`, `api_token`, `github_token`).

**What `GetSandboxProviderEnvironment` returns:** `map<string, string> environment` where:
- key = env var name the sandbox will see (e.g. `ANTHROPIC_API_KEY`)
- value = the placeholder string above (e.g. `openshell:resolve:env:v…_api_key`)

The placeholder is what appears in `printenv ANTHROPIC_API_KEY` inside the sandbox. The real
credential value is substituted by the proxy on egress — the sandbox never sees it.

**Implication for Task 7 (`get_sandbox_provider_environment`):** The function returns
`HashMap<String, String>` (env var → placeholder). Right may display these for diagnostics but
must NOT log them (per spec constraint).

**Verify in Task 10** using `ExecSandbox` gRPC to run `printenv <VAR>` and assert output starts
with `openshell:resolve:env:`.

---

## Additional findings (not in original steps)

### provider create CLI flags

```
openshell provider create --name <NAME> --type <TYPE> --credential KEY=VALUE [--credential KEY=VALUE...] [--config KEY=VALUE...]
openshell provider update <NAME> --credential KEY=VALUE [--config KEY=VALUE...]
openshell provider delete <NAME>
openshell provider get <NAME>
openshell provider list
```

- `--credential KEY=VALUE` passes both key and value in one flag.
- `--credential KEY` (no `=VALUE`) looks up the value from the environment.
- `--config KEY=VALUE` sets non-secret config fields (used for `generic` header_name, upstream_host, etc.).

**For gRPC `CreateProvider` and `UpdateProvider`:** The `Provider` message has:
- `metadata.name` — provider name
- `type` — type slug (e.g. `"generic"`, `"anthropic"`)
- `credentials: map<string, string>` — key = credential name (e.g. `"api_key"`, `"MY_TOKEN"`), value = secret
- `config: map<string, string>` — key = config name, value = non-secret

The gRPC path avoids the CLI entirely for CRUD — `CreateProvider`, `UpdateProvider`,
`DeleteProvider`, `GetProvider`, `ListProviders` are all available as gRPC RPCs.

**Decision for Tasks 5/6:**
- CRUD (create/rotate/delete/get/list) → gRPC only (no CLI).
- Attach/detach at runtime → CLI only (`openshell sandbox provider attach/detach`).
- Attach at creation time → `SandboxSpec.providers` field in `CreateSandboxRequest`.

### openshell settings CLI vs gRPC

`openshell settings set --global --key providers_v2_enabled --value true --yes` is the CLI form.
The `--yes` flag skips a confirmation prompt for global changes. When calling from Rust via gRPC
(`UpdateConfig`), no confirmation is needed.

### SandboxSpec.providers vs runtime attach

The existing live sandboxes (`test-sandbox-20260516-1640`, `test-sandbox-20260516-1649`) were created
before providers existed. They have no providers attached. The reconciler in Task 33 will need to
call `openshell sandbox provider attach` (CLI) to attach providers to sandboxes that already exist.

### `outlook` type

An `outlook` profile with zero credentials and zero endpoints exists on the gateway. Right should
hide it alongside `claude`. The spec only mentions hiding `claude` — add `outlook` to the hidden
set when building the profile catalog in Task 8.
