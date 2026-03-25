# Phase 11: Env Var Injection - Context

**Gathered:** 2026-03-25
**Status:** Ready for planning

<domain>
## Phase Boundary

Add `env:` key-value pairs to `AgentConfig` (agent.yaml), inject them into the generated shell wrapper before `exec claude`, fix the `installed.json` data-loss bug (unconditional overwrite → create-if-absent), and add a commented example + plaintext warning to the generated agent.yaml template.

No new CLI commands. No network calls. No process-compose changes. Pure Rust struct + codegen + template work.

</domain>

<decisions>
## Implementation Decisions

### Value Expansion (D-01)
- **D-01:** Strict literals only. All env: values are single-quoted in the generated wrapper: `export KEY='value'`. No `${VAR}` host expansion. What you write in agent.yaml is exactly what the agent gets.
- **D-02:** Corollary: users who need to forward a host env var must use `telegram_token_file` pattern (file reference) or set the value explicitly. Document this in the plaintext warning comment.

### Injection Ordering (D-03)
- **D-03:** env: vars are injected AFTER the 6 identity captures and BEFORE `export HOME=`. Template order:
  ```
  # identity captures (GIT_AUTHOR_NAME, GIT_AUTHOR_EMAIL, GIT_CONFIG_GLOBAL, SSH_AUTH_SOCK, GIT_SSH_COMMAND, ANTHROPIC_API_KEY)
  # env: vars from agent.yaml   ← new block here
  export HOME="{{ working_dir }}"
  ```
- **D-04:** env: can override the identity vars. If user sets `GIT_AUTHOR_NAME: "Bot"` in agent.yaml, the agent uses "Bot". Explicit agent config wins over defaults. Identity captures run first (so they're available as fallback), env: runs second (overrides if set).

### Quoting (D-05)
- **D-05:** Single-quote all injected values in the shell wrapper. Any single-quote characters in the value must be escaped using the standard bash trick: `'` → `'\''`. This is safe for all inputs including values with spaces, `$`, `"`, backticks.
- **D-06:** Fix existing `startup_prompt` quoting in the template (currently unescaped inside `""`). Out of scope if startup_prompt remains hardcoded in shell_wrapper.rs and doesn't accept user input — confirm during planning.

### installed.json Fix (D-07)
- **D-07:** Change `installed.json` write in `install_builtin_skills()` from unconditional `fs::write("{}")` to create-if-absent: write `{}` only when file does not exist. Follow `settings.local.json` pattern from Phase 9.
- **D-08:** The existing test `installs_installed_json` asserts content equals `"{}"` — this will need updating to verify create-if-absent behavior (write on first call, preserve existing on second call).

### Generated agent.yaml Template (D-09)
- **D-09:** The generated `agent.yaml` for new agents should include a commented env: example:
  ```yaml
  # env:
  #   MY_VAR: "literal value"  # plaintext only — do not store secrets here
  ```
  Placed after existing fields, before any other commented sections.

### Claude's Discretion
- Rust type for `env:` field: `IndexMap<String, String>` or `HashMap<String, String>`. Either works. Use whichever is already in workspace deps; add `indexmap` only if not already present.
- Whether to emit the env: block as a Jinja `{% for %}` loop or generate the export lines in Rust before passing to template — planner decides.
- Whether D-06 (startup_prompt quoting) is addressed in Phase 11 or deferred — hardcoded string, low risk, planner decides.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase Requirements
- `.planning/REQUIREMENTS.md` §Env Var Injection — ENV-01 through ENV-05

### Existing Implementation (read before touching)
- `crates/rightclaw/src/agent/types.rs` — AgentConfig struct with `deny_unknown_fields`; add `env` field here
- `crates/rightclaw/src/codegen/shell_wrapper.rs` — `generate_wrapper()` function; add `env` to context
- `templates/agent-wrapper.sh.j2` — shell wrapper template; add env: injection block between identity captures and HOME override
- `crates/rightclaw/src/codegen/skills.rs` — `install_builtin_skills()`; fix installed.json write on line 24
- `crates/rightclaw/src/codegen/shell_wrapper_tests.rs` — existing wrapper tests; add quoting tests here

### Prior Phase Decisions
- `.planning/phases/08-home-isolation-permission-model/08-CONTEXT.md` — D-01 through D-08 (HOME override, identity var ordering)
- `.planning/phases/09-agent-environment-setup/09-CONTEXT.md` — create-if-absent pattern for settings.local.json

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `SandboxOverrides` struct: reference for how to add a new nested section to `AgentConfig` — `#[serde(default)]`, `deny_unknown_fields` on the nested struct
- `settings.local.json` create-if-absent logic in Phase 9: exact pattern for D-07 fix

### Established Patterns
- `#[serde(deny_unknown_fields)]` on `AgentConfig` — adding `env:` REQUIRES adding the field to the struct; any agent.yaml with `env:` hard-fails deserialization until the field is added
- `#[serde(default)]` on optional fields — `env:` field should use this so agents without `env:` section parse fine
- minijinja `context!{}` macro in `generate_wrapper()` — new `env` key added here flows to template
- Telegram fields added in Phase 9: exact precedent for adding new optional fields to `AgentConfig`

### Integration Points
- `generate_wrapper()` in `shell_wrapper.rs` — receives `&AgentDef` which contains `config: Option<AgentConfig>`. Add env extraction: `agent.config.as_ref().map(|c| &c.env).unwrap_or_default()`
- `init.rs` `generate_agent_yaml()` (or equivalent) — where the agent.yaml template string lives; add commented env: example

</code_context>

<specifics>
## Specific Ideas

- User confirmed: single-quote quoting is correct — no host expansion wanted
- User confirmed: env: should win over identity captures (per-agent git identity use case acknowledged)
- Commented example in generated agent.yaml: `# MY_VAR: "literal value"  # plaintext only — do not store secrets here`

</specifics>

<deferred>
## Deferred Ideas

- Host env var forwarding/interpolation (${VAR} expansion) — explicitly rejected in D-01; if needed in future, add secretspec/vault (v2.3 SEED)
- startup_prompt quoting fix — likely low risk since it's hardcoded; planner assesses in Phase 11

None — discussion stayed within phase scope.

</deferred>

---

*Phase: 11-env-var-injection*
*Context gathered: 2026-03-25*
