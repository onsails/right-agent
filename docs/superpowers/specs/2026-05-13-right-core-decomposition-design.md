# Right-core decomposition for compile isolation

## Problem

`right-core` is currently the bottom-of-stack crate for unrelated concerns:
agent config DTOs, OpenShell integration, generated proto types, CLI UI,
runtime-state JSON, prompt-safety wrappers, STT helpers, process management,
platform-store deployment, test support, and UX/prose timing constants.

Because `right-core` is a dependency of `right-codegen`, `right-agent`,
`right-memory`, `right-bot`, and `right`, small edits to volatile pieces can
invalidate crates that do not use those pieces. The concrete trigger is
`time_constants`: a one-line idle-threshold/prose tweak currently sits in the
same crate as OpenShell, config, UI, and STT.

The design goal is not just cleaner names. The goal is measurable compile
isolation: edits should rebuild the owning crate and real direct consumers,
not everything that happens to depend on `right-core`.

## Goals

- Split `right-core` by ownership boundary and rebuild value.
- Make consumers import the owning crate directly.
- Avoid compatibility re-exports from `right-core`; they preserve old source
  paths but can reintroduce dependency edges and rebuild cascades.
- Preserve runtime behavior and user-facing compatibility.
- Keep each phase independently buildable, testable, and revertible.
- Update `ARCHITECTURE.md` and relevant `docs/architecture/*.md` as crate
  boundaries change.

## Non-goals

- No behavioral changes to cron delivery, STT, OpenShell, config parsing, or
  platform-store deployment.
- No prompt/prose edits beyond import/path-driven references.
- No historical rewrite of existing `docs/superpowers/specs/` or
  `docs/superpowers/plans/` files.
- No attempt to keep moved modules available at old `right_core::...` paths.

## Target crate boundaries

### `right-platform-knobs`

Owns low-level, agent-facing tunables that are expected to change more often
than foundation code.

Initial contents:

- `IDLE_THRESHOLD_SECS`
- `IDLE_THRESHOLD_MIN`

Dependencies: none beyond `std`.

### `right-prompt-safety`

Owns prompt-injection safety wrappers around external/untrusted content.

Initial contents:

- `sanitize_memory_content`
- `wrap_memory_for_prompt`
- `memory_wrap_prefix`
- `memory_wrap_suffix`
- `escape_memory_close_delimiter`

Dependencies: `ironclaw_safety` and `std`.

This lets `right-memory` depend on prompt-safety without depending on
OpenShell, config, UI, STT, or generated proto code.

### `right-agent-config`

Owns typed configuration and filesystem-discovered agent DTOs.

Initial contents:

- `AgentConfig`
- `AgentDef`
- `RestartPolicy`
- `SandboxConfig`
- `SandboxMode`
- `NetworkPolicy`
- `MemoryConfig`
- `MemoryProvider`
- `RecallBudget`
- `AttachmentsConfig`
- `SttConfig`
- `WhisperModel`

`WhisperModel` belongs here because it is part of `agent.yaml` schema. Putting
it in `right-stt` would make config and codegen consumers pull network/download
dependencies they do not need.

Normal dependencies: `serde` and `miette` for existing policy-path validation.
Dev dependencies: `serde-saphyr` for config parsing tests.

### `right-stt`

Owns host-side STT support helpers, not config schema.

Initial contents:

- `model_cache_path`
- `ffmpeg_available`
- `is_model_cached`
- `DownloadError`
- `download_model`
- `ensure_models_cached`

Dependencies: `right-agent-config`, `reqwest`, `futures`, `tokio`, `which`,
`thiserror`.

### `right-runtime-state`

Owns runtime state written by `right up` and read by `right down` or process
clients.

Initial contents:

- `PC_PORT`
- `MCP_HTTP_PORT`
- `RuntimeState`
- `AgentState`
- `write_state`
- `read_state`
- `generate_pc_api_token`

Dependencies: `serde`, `serde_json`, `miette`, `base64`, `rand`.

### `right-ui`

Owns CLI presentation primitives.

Initial contents:

- `ui::atoms`
- `ui::error`
- `ui::header`
- `ui::line`
- `ui::prompts`
- `ui::recap`
- `ui::splash`
- `ui::theme`
- `ui::writer`

Dependencies: `owo-colors`, `inquire`.

### `right-process`

Owns subprocess process-group handling.

Initial contents:

- `ProcessGroupChild`

Dependencies: `tokio`, `nix`.

### `right-openshell`

Owns OpenShell integration and generated proto types.

Initial contents:

- `openshell`
- generated `openshell_proto`
- `sandbox_exec`
- OpenShell live-test cleanup/support unless a later implementation pass proves
  a separate `right-openshell-test-support` crate is cleaner.

Dependencies: `right-process`, `tonic`, `tonic-prost`, `prost`, `prost-types`,
`serde-saphyr`, `fs4`, `tempfile`, `which`, `miette`, `tokio`, `tracing`,
`http`, `hyper-util`, and existing OpenShell-specific dependencies.

### `right-platform-store`

Owns content-addressed platform-managed file deployment into sandboxes.

Initial contents:

- `platform_store`

Dependencies: `right-openshell`, `sha2`, `walkdir`, `futures`, `tempfile`,
`miette`.

## Dependency-flow rules

- Consumers import moved modules from the owning crate directly.
- `right-core` does not re-export moved modules.
- Workspace dependencies are explicit in every consuming `Cargo.toml`.
- If a moved type appears in a public API, the owning crate is the public
  source of that type.
- Temporary duplicate definitions are not allowed.
- Back-edges are not allowed just to make compilation pass. If a move reveals
  a bad boundary, stop and adjust the boundary.

## Phased implementation

### Phase 1: low-risk compile wins

Move:

- `time_constants` to `right-platform-knobs`
- `injection_guard` to `right-prompt-safety`
- `runtime_state` to `right-runtime-state`

Update direct consumers and remove the moved modules from `right-core`.

Expected wins:

- Editing idle/prose timing constants rebuilds `right-platform-knobs` and
  direct consumers such as `right-codegen`, `right-agent`, and `right-bot`, not
  `right-core`, `right-db`, `right-mcp`, or `right-memory`.
- Prompt-safety changes no longer force memory code through the full
  `right-core` dependency set.
- Runtime-state edits no longer share a crate with OpenShell/proto/UI/STT.

### Phase 2: config and STT split

Move:

- `agent_types` to `right-agent-config`
- `WhisperModel` with the config DTOs
- STT download/cache/ffmpeg helpers to `right-stt`

Update `right-codegen`, `right-agent`, `right-bot`, and `right` imports.

Expected wins:

- Config/schema edits rebuild config consumers, not OpenShell/proto/platform
  store.
- STT download helper edits rebuild STT consumers only.

### Phase 3: CLI, process, OpenShell, and platform store split

Move:

- `ui` to `right-ui`
- `process_group` to `right-process`
- `openshell`, `openshell_proto`, `sandbox_exec`, `test_cleanup`, and
  `test_support` to `right-openshell`
- `platform_store` to `right-platform-store`

Expected wins:

- CLI presentation edits stop rebuilding bot/MCP/memory crates that do not
  import UI.
- OpenShell/proto edits are isolated to CLI, agent, bot, and OpenShell tests.
- `right-memory` and `right-codegen` should not depend on OpenShell unless they
  use an OpenShell API directly.

### Phase 4: shrink or remove `right-core`

After moved imports have settled, inspect remaining `right-core` contents.
Preferred end state: remove `right-core` from the normal workspace dependency
graph. If anything remains, it needs a concrete foundation role and a measured
reason to stay.

## Error handling

The migration preserves existing error behavior.

- Do not change error types or messages unless a crate move requires a path
  update.
- Existing `miette::Result` APIs can stay `miette::Result` during this
  refactor.
- `right-stt` keeps `DownloadError` as a `thiserror` type.
- No moved boundary may silently swallow errors.
- No broad facade APIs should hide dependency mistakes.

## Testing and verification

Use TDD per phase.

For each phase:

1. Start with a focused failing check that proves the old boundary is being
   removed, or use an existing compile/test failure as the regression signal.
2. Move the module into the new crate.
3. Update imports and manifests.
4. Run package-level tests for the moved code and direct consumers.
5. Run `devenv shell -- cargo build --workspace`.
6. Run `devenv shell -- cargo test --workspace` at the end of the phase.
7. Probe rebuild fan-out by changing one moved constant/function body, running
   `devenv shell -- cargo build --workspace -v`, checking compiled crate names,
   then reverting the probe edit.

Phase-specific checks:

- Phase 1: `cargo test -p right-codegen`, `cargo test -p right`,
  `cargo test -p right-memory`, and idle-threshold fan-out.
- Phase 2: `cargo test -p right-codegen`, `cargo test -p right-agent`,
  `cargo test -p right-bot`, and STT/config tests.
- Phase 3: OpenShell and test-support moves must keep live sandbox tests
  runnable; do not mark them ignored.
- Final: `cargo build --workspace` and `cargo test --workspace`.

## Acceptance criteria

- `right-core` no longer owns volatile prompt/prose constants after Phase 1.
- `right-memory` has no normal dependency on the full `right-core` crate after
  prompt-safety is moved.
- Moved modules are imported from their owning crates, not through
  `right-core`.
- Rebuild fan-out probes show each moved crate only rebuilding expected direct
  consumers.
- `ARCHITECTURE.md` and `docs/architecture/modules.md` match the live crate map
  after each phase.
- `docs/architecture/sessions.md` is updated when the idle-threshold constant
  moves because that document names the constant.
