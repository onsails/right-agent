---
phase: 02-cli-runtime-and-sandboxing
plan: 01
subsystem: codegen
tags: [minijinja, process-compose, openshell, templates, codegen]

requires:
  - phase: 01-project-bootstrap
    provides: "AgentDef, AgentConfig, RestartPolicy types and agent discovery"
provides:
  - "Shell wrapper generation (generate_wrapper) for sandbox and no-sandbox modes"
  - "process-compose.yaml generation (generate_process_compose) from agent list"
  - "Jinja2 templates for both artifacts (agent-wrapper.sh.j2, process-compose.yaml.j2)"
  - "Phase 2 workspace deps: minijinja, reqwest, tokio, serde_json, which"
affects: [02-02, 02-03, runtime, cli-commands]

tech-stack:
  added: [minijinja 2.18, reqwest 0.13, tokio 1.50, serde_json 1.0, which 7.0]
  patterns: [include_str! template embedding, minijinja context rendering, TDD with separate test files]

key-files:
  created:
    - templates/agent-wrapper.sh.j2
    - templates/process-compose.yaml.j2
    - crates/rightclaw/src/codegen/mod.rs
    - crates/rightclaw/src/codegen/shell_wrapper.rs
    - crates/rightclaw/src/codegen/process_compose.rs
    - crates/rightclaw/src/codegen/shell_wrapper_tests.rs
    - crates/rightclaw/src/codegen/process_compose_tests.rs
    - crates/rightclaw/src/runtime/mod.rs
  modified:
    - Cargo.toml
    - crates/rightclaw/Cargo.toml
    - crates/rightclaw-cli/Cargo.toml
    - crates/rightclaw/src/lib.rs

key-decisions:
  - "reqwest feature 'rustls' (not 'rustls-tls') for 0.13 compatibility"
  - "include_str! paths use ../../../../ from codegen/ to reach templates/ at repo root"

patterns-established:
  - "Codegen via minijinja: embed template with include_str!, render with typed context"
  - "RestartPolicy mapping: OnFailure -> on_failure, Always -> always, Never -> no"

requirements-completed: [CLI-01, SAND-01, SAND-02]

duration: 4min
completed: 2026-03-22
---

# Phase 02 Plan 01: Codegen Module Summary

**Minijinja-based codegen for shell wrappers (openshell sandbox + no-sandbox) and process-compose.yaml from AgentDef list, with 15 tests**

## Performance

- **Duration:** 4 min
- **Started:** 2026-03-22T16:49:43Z
- **Completed:** 2026-03-22T16:53:34Z
- **Tasks:** 2
- **Files modified:** 15

## Accomplishments
- Workspace deps added for all Phase 2 needs (minijinja, reqwest, tokio, serde_json, which)
- Shell wrapper template generates correct openshell sandbox invocation or direct claude exec
- process-compose.yaml template generates valid config with restart policies, backoff, shutdown
- 15 tests covering both codegen functions across all variants

## Task Commits

Each task was committed atomically:

1. **Task 1: Add Phase 2 workspace deps, templates, and codegen module structure** - `ba13116` (feat)
2. **Task 2: Implement codegen functions with TDD** - `3f2dfb7` (test)

## Files Created/Modified
- `templates/agent-wrapper.sh.j2` - Shell wrapper template with openshell/no-sandbox branches
- `templates/process-compose.yaml.j2` - process-compose.yaml template with agent loop
- `crates/rightclaw/src/codegen/mod.rs` - Module re-exports for generate_wrapper and generate_process_compose
- `crates/rightclaw/src/codegen/shell_wrapper.rs` - generate_wrapper function using minijinja
- `crates/rightclaw/src/codegen/process_compose.rs` - generate_process_compose function with ProcessAgent context
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` - 7 tests for wrapper generation
- `crates/rightclaw/src/codegen/process_compose_tests.rs` - 8 tests for PC config generation
- `crates/rightclaw/src/runtime/mod.rs` - Placeholder for Plan 02
- `Cargo.toml` - Workspace deps added
- `crates/rightclaw/Cargo.toml` - Library crate deps added
- `crates/rightclaw-cli/Cargo.toml` - tokio dep added for future #[tokio::main]
- `crates/rightclaw/src/lib.rs` - codegen and runtime modules declared

## Decisions Made
- reqwest 0.13 renamed `rustls-tls` feature to `rustls` -- fixed during build
- include_str! path from `crates/rightclaw/src/codegen/` to `templates/` requires 4 levels up (`../../../../`)
- Implementation written alongside skeleton in Task 1 (templates + functions), tests added in Task 2 -- all passed first run

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed reqwest feature name for 0.13**
- **Found during:** Task 1 (cargo check)
- **Issue:** Plan specified `features = ["json", "rustls-tls"]` but reqwest 0.13 renamed it to `rustls`
- **Fix:** Changed feature to `rustls` in workspace Cargo.toml
- **Files modified:** Cargo.toml
- **Verification:** cargo check --workspace succeeds
- **Committed in:** ba13116 (Task 1 commit)

**2. [Rule 3 - Blocking] Fixed include_str! path depth**
- **Found during:** Task 1 (cargo check)
- **Issue:** Plan suggested `../../../templates/` but correct relative path from codegen/ is `../../../../templates/`
- **Fix:** Added extra `../` to both include_str! invocations
- **Files modified:** shell_wrapper.rs, process_compose.rs
- **Verification:** cargo check --workspace succeeds
- **Committed in:** ba13116 (Task 1 commit)

---

**Total deviations:** 2 auto-fixed (2 blocking)
**Impact on plan:** Both were incorrect paths/names in the plan. No scope creep.

## Issues Encountered
None beyond the auto-fixed deviations above.

## Known Stubs
None -- all codegen functions are fully wired to templates and produce complete output.

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Codegen module ready for `rightclaw up` to call generate_wrapper and generate_process_compose
- Runtime module placeholder exists for Plan 02 (pc_client, sandbox management)
- tokio added to CLI crate for async main in Plan 03

---
*Phase: 02-cli-runtime-and-sandboxing*
*Completed: 2026-03-22*
