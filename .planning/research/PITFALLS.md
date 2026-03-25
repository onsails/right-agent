# Pitfalls Research: v2.2 Skills Registry

**Domain:** Registry integration replacement (ClawHub → skills.sh), env var injection into agent shell wrappers, skill manager UX, CC-native sandbox interaction with injected vars
**Researched:** 2026-03-25
**Confidence:** HIGH (codebase audited, current wrapper/codegen inspected, seeds read, CC sandbox behavior verified from v2.1 research)

---

## Critical Pitfalls

### Pitfall 1: Env Var Values With Spaces/Special Chars Break the Bash Wrapper

**What goes wrong:**
The current wrapper template (`agent-wrapper.sh.j2`) emits shell variable assignments rendered by minijinja. If `agent.yaml` contains:

```yaml
env:
  MY_PROMPT: "Hello, world! I am the agent."
```

And the template renders it as:

```bash
export MY_PROMPT=Hello, world! I am the agent.
```

The shell splits on spaces and the comma. Even with:

```bash
export MY_PROMPT="{{ value }}"
```

minijinja's default auto-escaping is OFF for non-HTML templates, so a value containing `"` (double quote) or `\` (backslash) or `$` breaks the shell assignment. An adversarial skill author who tricks a user into setting:

```yaml
env:
  EVIL: "\"; exec malicious_binary #"
```

gets arbitrary command execution at wrapper startup (before `exec claude`), entirely outside the CC sandbox — the wrapper runs unconfined.

**Why it happens:**
Shell quoting is subtly different from YAML string escaping. Values that round-trip cleanly through YAML still break shell. The template has no mechanism to shell-escape values. The existing wrapper already handles `startup_prompt` by passing it through unquoted in `-- "{{ startup_prompt }}"` — same risk exists there.

**How to avoid:**
- In `generate_wrapper()`, shell-escape all user-supplied values before injecting into the template. Use `shlex`-style escaping: wrap each value in single quotes and replace any `'` in the value with `'\''`.
- In Rust: implement a `shell_quote(s: &str) -> String` helper: `format!("'{}'", s.replace('\'', "'\\''"))`.
- Never emit user-supplied values directly into the template without escaping.
- The `startup_prompt` variable has the same problem — fix it in the same pass.

**Warning signs:**
- Test: set `env: { VAR: "hello world" }` and inspect the generated wrapper. If the line reads `export VAR=hello world` (no quotes), the bug is present.
- Test: set `env: { VAR: "it's fine" }` — if the apostrophe breaks the script, the bug is present.
- `set -euo pipefail` at the top of the wrapper will cause `exit 1` silently on a bad export, making the agent appear to crash on launch for no obvious reason.

**Phase to address:** Phase 1 (env var injection). Must be solved in the same commit that adds `env:` support. Never ship user value injection without shell quoting.

---

### Pitfall 2: Env Vars Injected AFTER the HOME Override Lose Host Values

**What goes wrong:**
The current wrapper structure is:

```bash
# 1. Capture identity vars from host env BEFORE HOME override
export GIT_CONFIG_GLOBAL="${GIT_CONFIG_GLOBAL:-}"
export ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}"
# ...

# 2. Override HOME
export HOME="{{ working_dir }}"

# 3. exec claude
exec "$CLAUDE_BIN" ...
```

If `env:` vars are injected between step 2 and step 3 (the obvious "append to bottom" placement), vars that reference `$HOME` in their values resolve to the agent dir, not the host home:

```yaml
env:
  CUSTOM_CACHE: "$HOME/.cache/myapp"  # user means /home/wb/.cache/myapp
```

After HOME override, `$HOME/.cache/myapp` resolves to `<agent_dir>/.cache/myapp`. The agent writes to its own dir — probably intentional. But for vars like:

```yaml
env:
  GIT_CONFIG_GLOBAL: "$HOME/.gitconfig"
```

...this silently overrides the already-captured `GIT_CONFIG_GLOBAL` and points it at a non-existent path in the agent dir.

More critically: if a user puts `ANTHROPIC_API_KEY` in `env:`, it must appear BEFORE the HOME override section (where the existing capture happens) to correctly chain. But if it's injected after, both the capture line AND the user's line exist — the user's line wins (last assignment wins in bash), but the ordering is confusing.

**Why it happens:**
The wrapper has a specific ordering contract for HOME isolation (v2.1 design). `env:` injection is a new feature that was not part of that design, so there is no obvious "correct" insertion point.

**How to avoid:**
- Inject user `env:` vars AFTER the HOME override block, clearly documented.
- Expand `$HOME` references in env var values at codegen time in Rust (before writing the wrapper), replacing `$HOME` with the actual agent path. This makes the wrapper self-documenting and avoids runtime expansion surprises.
- Alternatively, emit `env:` vars with single-quoted values (preventing `$HOME` expansion entirely) and document that `$HOME` expansion is not supported in `env:` values.
- Add a validation step: warn if any user env key conflicts with the six identity vars already exported (`GIT_CONFIG_GLOBAL`, `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`, `SSH_AUTH_SOCK`, `GIT_SSH_COMMAND`, `ANTHROPIC_API_KEY`). Override is allowed but emit a warning at `rightclaw up` time.

**Warning signs:**
- Test: set `env: { ANTHROPIC_API_KEY: "sk-test" }` and inspect generated wrapper — do two `export ANTHROPIC_API_KEY=...` lines appear?
- Test: agent dir path appears in an env var that should point to the host filesystem.

**Phase to address:** Phase 1 (env var injection). Ordering contract must be explicit in the template design doc and enforced in codegen.

---

### Pitfall 3: `deny_unknown_fields` on `AgentConfig` Blocks Migration for Existing Users

**What goes wrong:**
`AgentConfig` is deserialized with `#[serde(deny_unknown_fields)]`. This is correct for validation — it catches typos. But when v2.2 adds `env:` field to `AgentConfig`, any existing user who:
1. Has an `agent.yaml` without an `env:` section — works fine (field is `Option` or `default`).
2. Upgrades rightclaw but has a custom field in their `agent.yaml` that was never in the spec — **their agent fails to load** with "unknown field" error.

The second case is not hypothetical. Users copy-paste agent.yaml from OpenClaw examples, which may have additional fields (e.g., `version:`, `tags:`, `description:`) that ClawHub uses but RightClaw never exposed. These users currently get the "unknown field" error already (pre-existing). But v2.2 is the first release where this could regress a working setup: if a user added a comment field workaround for something (e.g., `_env_note: "see .env"`) and their agent was silently ignoring it (impossible given deny_unknown_fields, but they think it works), adding `env:` doesn't help them.

More practically: v2.2 adds `env:` as a first-class field. Users who try `env:` on an older rightclaw build (before upgrade) get "unknown field" error — which is correct but confusing. The error message from serde-saphyr says `unknown field 'env' in AgentConfig` which is not actionable without docs.

**Why it happens:**
`deny_unknown_fields` is good hygiene but produces opaque errors. The error message doesn't say "upgrade rightclaw" or "check docs."

**How to avoid:**
- Add `env` field to `AgentConfig` with `#[serde(default)]` so agents without `env:` continue to work.
- Keep `deny_unknown_fields` — it's correct.
- When `AgentConfig` deserialization fails, wrap the error with miette context that says "check your agent.yaml against the schema at <docs URL>".
- In `rightclaw doctor`: validate all agent.yaml files and report schema errors with actionable guidance, not just at `rightclaw up` time.

**Warning signs:**
- Integration test `test_init_creates_agent` passes but real-world agents with extra fields fail.
- User reports "unknown field 'env'" after editing their agent.yaml — this means the new field name clashes with something they have.

**Phase to address:** Phase 1 (AgentConfig extension). The `env:` field addition must be backward-compatible.

---

### Pitfall 4: `installed.json` Tracks Two Registries Differently — `list` Output Diverges From Reality

**What goes wrong:**
The current `/skills` skill (in `skills/clawhub/SKILL.md`) tracks installed skills in `.claude/skills/installed.json` with a `"source": "skills.sh"` field. When v2.2 adds ClawHub as a secondary/fallback registry, skills installed from ClawHub will have `"source": "clawhub"`. The `list` command scans `.claude/skills/` for directories containing `SKILL.md` AND reads `installed.json`.

Problems:
1. Skills installed via the OLD `/clawhub` skill (before v2.2) have `"source": "clawhub"` in their `installed.json`. After upgrade, the new `/skills` skill reads the same file — no migration. These entries show up correctly.
2. Skills installed manually (no `installed.json` entry) show `"source": "manual"` — fine.
3. The NEW edge case: a skill installed from skills.sh is removed, reinstalled from ClawHub. `installed.json` has two approaches but one directory — if the install step doesn't update the existing entry, you get stale source metadata.
4. More critically: `install_builtin_skills()` in Rust always writes `{}` to `installed.json` — it **resets the registry** on every `rightclaw up` call. Line from `codegen/skills.rs`:

   ```rust
   ("clawhub/SKILL.md", SKILL_CLAWHUB),
   ```

   ...is being renamed but `install_builtin_skills()` also writes `installed.json` with `{}`. Any skills tracked by the agent in `installed.json` (installed by the user via `/skills install`) are **wiped on every `rightclaw up`**.

This is the most severe bug in the current design: `rightclaw up` reinstalls built-in skills (correct) but also resets `installed.json` to `{}` (incorrect). After v2.2, users install skills via `/skills install`, which writes to `installed.json`. On next `rightclaw up`, that file is reset.

**Why it happens:**
`install_builtin_skills()` was written to create the file if absent (correct) but unconditionally overwrites it (incorrect). The original intent was "idempotent" but the implementation is "destructive."

**How to avoid:**
- Fix `install_builtin_skills()` to only write `installed.json` if the file does NOT exist (create-if-absent semantics).
- Never overwrite `installed.json` from Rust codegen — it is agent-owned state.
- Write a regression test: create `installed.json` with some content, call `install_builtin_skills()`, verify content is unchanged.
- In the SKILL.md skill update: document that `installed.json` is the source of truth for user-installed skills and must not be overwritten by the runtime.

**Warning signs:**
- After `rightclaw up`, `npx skills list` shows no user-installed skills.
- User reports "I installed skill X yesterday but it's gone today."
- The `installed.json` always contains `{}` even after installing skills.

**Phase to address:** Phase 1 (install_builtin_skills fix). This is a data-loss bug that must be fixed before v2.2 ships any user-facing `/skills install`.

---

### Pitfall 5: Policy Gate Checks OpenShell `metadata.openclaw` — Silently Passes All skills.sh Skills

**What goes wrong:**
The current `/skills` SKILL.md has a policy gate (Step 3 of install) that checks `metadata.openclaw.requires.*` frontmatter fields. skills.sh skills use the agentskills.io format, which does NOT use `metadata.openclaw`. The SKILL.md policy check reads those fields — but for skills.sh skills, those fields are absent. The gate evaluates to "no requirements found" → "all checks pass" → install proceeds.

This means the policy gate is a no-op for the primary registry. Users get false assurance that skills are audited before install. A skill from skills.sh could require `BROWSER_USE_API_KEY` (env var), Chrome binary, and network access to `api.openai.com` — none of which the agent has — but the gate passes.

The v2.2 plan says "Policy gate reworked — drop OpenShell/policy.yaml refs, check CC-native sandbox capabilities." This is correct but needs to cover agentskills.io format, not just openclaw format.

**Why it happens:**
The policy gate was written for ClawHub/OpenClaw skills. The migration to skills.sh was done in SKILL.md content but the policy gate logic was not updated.

**How to avoid:**
- Rework the gate to parse agentskills.io SKILL.md frontmatter fields (if the spec defines requirements). Check the agentskills.io specification for requirement syntax.
- At minimum, add a heuristic check: read the SKILL.md body for `env:` references or bash commands that suggest external binaries or services, and warn the user.
- Check CC-native sandbox capabilities: read the agent's `.claude/settings.json` and verify that `allowedDomains` includes any domains the skill documentation mentions.
- Document the limitation clearly: "Policy gate checks CC-native sandbox settings and agentskills.io metadata. Skills with undocumented requirements will pass the gate."

**Warning signs:**
- The gate shows "All checks passed" for a skill that clearly requires Node.js and an API key.
- The gate only ever shows requirements for skills with `metadata.openclaw:` in their frontmatter (i.e., only ClawHub skills trigger it).

**Phase to address:** Phase 2 (policy gate rework). The gate should be reworked to be format-agnostic.

---

### Pitfall 6: Env Vars Containing Secrets Leak Into `process-compose` REST API

**What goes wrong:**
process-compose exposes a REST API on `localhost:8080` (or the configured port). The `/process/info` and `/process/list` endpoints return the full process configuration including environment variables. Any env var set in the wrapper script via `export` is inherited by the `process-compose` child processes and captured in its memory. The REST API leaks them.

In practice, the wrapper uses `exec` to replace itself with `claude`, so env vars are in the `claude` process, not the wrapper. But process-compose captures env vars at process start, before `exec`. The env snapshot in process-compose's memory contains everything exported before `exec claude`.

If `env:` vars include `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or any other secret the user injects, those secrets are accessible to anyone who can reach `localhost:8080`. This is already a risk with the existing `ANTHROPIC_API_KEY` capture — v2.2 makes it worse by encouraging users to inject more secrets.

**Why it happens:**
The TUI design of process-compose shows env vars for each process as a feature. What's useful for debugging is dangerous for secrets. The existing wrapper already has `export ANTHROPIC_API_KEY=...` — v2.2 adds more.

**How to avoid:**
- For secrets: prefer reading from a file at wrapper startup (`export SECRET=$(cat "$AGENT_DIR/.secret_name")`) rather than embedding literal values in the generated wrapper. The file can have 0600 permissions.
- Document in agent.yaml schema: `env:` values that start with `$` are expanded from host environment at wrapper startup (not embedded in the script). The wrapper should emit `export VAR="${VAR:-}"` style (forward from host) rather than literal values.
- For the `ANTHROPIC_API_KEY` already in the wrapper: add a comment noting this limitation; consider the `apiKeyHelper` alternative.
- Rate-limit or auth-gate the process-compose REST API port if running on a shared machine.
- Note: the existing `ANTHROPIC_API_KEY` capture in the wrapper uses `"${ANTHROPIC_API_KEY:-}"` which forwards the host value — that's the correct pattern for secrets.

**Warning signs:**
- `curl http://localhost:8080/process/info` returns env vars including secrets.
- process-compose TUI shows `ANTHROPIC_API_KEY=sk-ant-...` in the process detail pane.

**Phase to address:** Phase 1 (env var injection). The `env:` feature documentation must describe the secret leakage risk and recommend the forwarding-from-host pattern over literal embedding.

---

## Moderate Pitfalls

### Pitfall 7: CC Sandbox Blocks `npx` — the Primary Skills CLI

**What goes wrong:**
The `/skills` skill uses `npx skills add <slug>` as its primary install mechanism. `npx` requires:
1. Network access to `registry.npmjs.org` (to download the `skills` package)
2. Write access to npm's cache directory (typically `~/.npm/` — which under HOME override resolves to `<agent_dir>/.npm/`)
3. Write access to a temp directory for extraction

The current CC native sandbox (`settings.json` generated by `generate_settings()`) has a default-deny network policy with an explicit `allowedDomains` list. `registry.npmjs.org` is NOT in the default list. The Bash tool runs under sandbox constraints (bubblewrap on Linux), so `npx skills add` fails silently or with a cryptic network error.

The SKILL.md has a fallback to `git clone` — which requires:
1. Network access to `github.com` — also NOT in the default list
2. SSH or HTTPS access

So both primary and fallback mechanisms fail inside the default sandbox.

**Why it happens:**
The skill was designed before v2.0 sandbox was the enforcement layer. Network defaults were not updated.

**How to avoid:**
- Add `registry.npmjs.org`, `github.com`, and `api.github.com` to the default `allowed_domains` list in `generate_settings()`, OR document that agents using `/skills install` need to add those domains to their `sandbox.allowed_domains` in `agent.yaml`.
- A middle ground: add a comment in the generated `settings.json` listing the domains needed for skill management, and update the doctor check to warn if those domains are absent and the skills skill is installed.
- The SKILL.md should document this: "Requires `registry.npmjs.org` and `github.com` in your agent's `sandbox.allowed_domains`."

**Warning signs:**
- `/skills install vercel-labs/some-skill` returns "network error" or "command not found for npx"
- `npx skills find` silently returns no results (network blocked, treated as empty response)
- The `git clone` fallback in SKILL.md fails with "Could not resolve host"

**Phase to address:** Phase 1 or 2. Either add the domains to defaults or document as a required manual override.

---

### Pitfall 8: `SKILL_CLAWHUB` Const Rename Breaks Idempotency Check in Tests

**What goes wrong:**
`codegen/skills.rs` has `const SKILL_CLAWHUB: &str = include_str!("../../../../skills/clawhub/SKILL.md")`. The rename in v2.2 (to `skills` or `rightskills`) requires:
1. Renaming the directory `skills/clawhub/` to `skills/skills/` (or `skills/rightskills/`)
2. Renaming `SKILL_CLAWHUB` const
3. Updating `install_builtin_skills()` to write `skills/SKILL.md` instead of `clawhub/SKILL.md`
4. Updating the install tests that assert `clawhub/SKILL.md` exists
5. Updating `init.rs` print statements and test assertions (lines 173, 250 assert `clawhub/SKILL.md`)

If any of these five locations are updated but not all, the build may succeed (if the old directory still exists) but deployed skill will be the old file, or tests will fail on the new path while the binary installs to the old path.

**Why it happens:**
The skill path appears in four different places: the filesystem (skills/ dir), the Rust const, the install function, and test assertions. A rename is a refactor, not a one-line change.

**How to avoid:**
- Do the rename atomically: `rg -l "clawhub"` to find all occurrences, update all in one commit.
- After rename: `cargo test` will catch any missed path. The `installs_clawhub_skill` test will fail if the path is wrong.
- The install is idempotent by design — the old `clawhub/SKILL.md` directory from previous runs will stay on disk. Add cleanup: `rightclaw up` should remove the old `clawhub/` directory if the new `skills/` directory now exists (migration step, one-time).

**Warning signs:**
- After rename, `rightclaw up` still installs to `.claude/skills/clawhub/` because the old path was not updated in `install_builtin_skills()`.
- Old agents have BOTH `.claude/skills/clawhub/SKILL.md` AND `.claude/skills/skills/SKILL.md` — two versions of the skill, both active, one stale.

**Phase to address:** Phase 1 (rename). Simple but must be complete across all four locations.

---

### Pitfall 9: ClawHub Backward Compatibility Adds API Ambiguity

**What goes wrong:**
The v2.2 plan keeps ClawHub as secondary/fallback. The `/skills` skill will call skills.sh first, then fall back to ClawHub. But:

1. skills.sh returns skills in agentskills.io format. ClawHub returns skills with `metadata.openclaw`. If the skill name exists in both registries, the user gets the skills.sh version (no policy gate for openclaw requirements) — this could silently install an under-featured version.

2. ClawHub had the ClawHavoc incident (341 malicious skills). RightClaw's fallback to ClawHub means users who don't find a skill on skills.sh are automatically exposed to the larger, less-curated, post-incident-but-not-fully-cleaned ClawHub.

3. The skills.sh `npx skills add` CLI downloads from GitHub. ClawHub's fallback path likely uses a different API. The SKILL.md needs to clearly separate "primary path (npx)" from "fallback path (ClawHub API direct)".

**Why it happens:**
The fallback design conflates two different trust levels (curated vs. post-incident) into a single user-facing flow. Users see "install skill" without knowing which registry served it.

**How to avoid:**
- Be explicit in the skill output: show which registry served the result. "Installing from skills.sh (verified)" vs "Installing from ClawHub (unverified, requires permission review)."
- For ClawHub installs, always run the full `metadata.openclaw` policy gate — do not skip it just because the gate is a partial check.
- Consider requiring explicit `--registry clawhub` to use ClawHub rather than auto-fallback. The security incident justifies opting out of automatic fallback.
- Document the registry trust hierarchy in SKILL.md.

**Warning signs:**
- User sees "Installed from ClawHub" but assumed they were getting the skills.sh version.
- ClawHub fallback happens silently — user doesn't know which registry served the skill.

**Phase to address:** Phase 2 (ClawHub fallback design). The fallback UX should be explicit, not silent.

---

### Pitfall 10: Skills Installed From skills.sh Survive `rightclaw up` But `installed.json` Is Reset (See Pitfall 4)

This is the secondary consequence of Pitfall 4. Even though the skill files survive in `.claude/skills/<name>/` (because `install_builtin_skills()` only overwrites `clawhub/SKILL.md` and `rightcron/SKILL.md`), the `installed.json` registry is reset to `{}`. This means:

1. `list` shows skills as "manual" (found on disk, not in registry) even though they were installed via `/skills install`.
2. `update` via `npx skills update` may not know which skills to update (no registry).
3. `remove` via the CLI may fail if `installed.json` doesn't have the entry.

This reinforces the Pitfall 4 fix: `install_builtin_skills()` must never overwrite `installed.json` if it already exists.

**Phase to address:** Phase 1 (same fix as Pitfall 4).

---

## Technical Debt Patterns

| Shortcut | Immediate Benefit | Long-term Cost | When Acceptable |
|----------|-------------------|----------------|-----------------|
| Emit env vars without shell quoting | Simpler template | Security vulnerability (command injection) + silent crashes for values with spaces | Never |
| Forward all `agent.yaml` `env:` vars to process-compose YAML environment section | Easier to debug (visible in TUI) | Secrets visible in REST API and TUI | Never for secrets; acceptable for non-sensitive debug vars if documented |
| Keep `metadata.openclaw` policy gate as-is for ClawHub skills | No new code | Gate is a no-op for skills.sh (primary registry) | Acceptable as temporary measure if labeled "ClawHub only" |
| Auto-fallback to ClawHub without user consent | More skills findable | Exposes users to post-ClawHavoc registry without warning | Never without explicit user opt-in |
| Expand `$HOME` in env values at runtime (let bash do it) | Zero codegen effort | HOME resolves to agent dir after HOME override — confusing, potentially wrong | Never; expand or quote at codegen time |
| Write `installed.json` as `{}` on every `rightclaw up` | Guaranteed clean state | Data loss of user-installed skills registry | Never |
| Inject env vars literally into wrapper script | Readable generated output | Secrets in wrapper file on disk; visible if file permissions are lax | Only for non-sensitive, non-secret vars |

## Integration Gotchas

| Integration | Common Mistake | Correct Approach |
|-------------|----------------|------------------|
| skills.sh `npx skills add` | Assuming `npx` and npm registry are accessible inside CC sandbox | Add `registry.npmjs.org`, `github.com` to `allowed_domains` or document as prerequisite |
| CC sandbox + env vars | Assuming injected env vars are visible inside sandboxed Bash | They are — env vars pass through to bubblewrap child. But the sandbox may block what those vars point to (e.g., a path outside allowRead). |
| ClawHub API | Sending unauthenticated requests after ClawHavoc | Check if ClawHub added auth/rate limiting post-incident; handle 429/403 explicitly |
| agentskills.io SKILL.md format | Reading `metadata.openclaw.*` fields on skills.sh skills | Parse standard YAML frontmatter only; `metadata.openclaw` is absent for skills.sh skills |
| `installed.json` | Assuming it reflects all installed skills | Skills installed via `npx skills add` directly (bypassing the SKILL.md skill) won't be in `installed.json`; the `list` command handles this via disk scan |
| process-compose REST API | Putting secrets in process-compose YAML `environment:` | Use wrapper `export VAR="${VAR:-}"` forwarding instead; secrets stay out of PC's config |
| minijinja template | Not escaping user values | Use shell quoting helper in Rust before passing to template context |

## Security Mistakes

| Mistake | Risk | Prevention |
|---------|------|------------|
| Injecting `agent.yaml` `env:` values directly into wrapper without shell quoting | Command injection at wrapper startup, outside CC sandbox | Shell-escape all user values with `'...'` quoting and `'\''` for embedded single quotes |
| Embedding secret values literally in generated wrapper script | Secret on disk in `~/.rightclaw/agents/<name>/run.sh` (agent state dir); visible if permissions are lax | Use `export VAR="${VAR:-}"` to forward from host env; document "set secrets in host env, reference via $VAR in agent.yaml" |
| Silent fallback to ClawHub without showing trust level | User installs a post-ClawHavoc skill believing it came from the curated skills.sh | Show source registry in all install confirmations; require explicit opt-in for ClawHub |
| Skipping policy gate for skills.sh skills because `metadata.openclaw` is absent | Skills with undocumented network/binary requirements install silently, then fail at runtime | Gate should check CC-native sandbox settings regardless of skill format |
| `env:` values referencing `$HOME` after HOME override | Paths resolve to agent dir instead of host home | Expand or prohibit `$HOME` references in env values; document the override behavior |

## UX Pitfalls

| Pitfall | User Impact | Better Approach |
|---------|-------------|-----------------|
| `npx` fails silently inside sandbox — no clear error | User runs `/skills install`, gets no output or cryptic "network error" | Check if `registry.npmjs.org` is in allowed_domains before attempting install; emit actionable error |
| `installed.json` reset on `rightclaw up` | Installed skills appear as "manual" in list; update/remove may fail | Fix Pitfall 4; never overwrite user-owned registry |
| Registry not shown in install confirmation | User doesn't know if skill came from skills.sh or ClawHub | Always show "Installed from skills.sh" or "Installed from ClawHub" |
| Version pinning not supported | Updating a skill always gets latest; no rollback | Track `version` field in `installed.json` entry; show diff before update |
| Offline install fails completely | Developer environments with restricted outbound may not reach skills.sh or GitHub | Document `git clone` manual fallback with exact steps; provide `rightclaw skill bundle` as future work |
| `/clawhub` skill still present after rename | Users with `/clawhub` in their prompts or memory get a skill not found error | Provide migration: `rightclaw up` replaces old `clawhub/` dir with new `skills/` dir; old prompts still work if /clawhub is an alias in the new skill |

## "Looks Done But Isn't" Checklist

- [ ] **Shell quoting:** Env var values with spaces, quotes, and special characters — verify the generated wrapper is valid bash with `bash -n run.sh`
- [ ] **installed.json preservation:** Run `rightclaw up` twice after installing a skill — verify `installed.json` still has the entry after the second `up`
- [ ] **HOME ordering:** Inject an env var that references `$HOME` — verify it resolves as the user intended (agent dir vs. host home)
- [ ] **Conflict warning:** Add `ANTHROPIC_API_KEY` to `env:` — verify `rightclaw up` warns about the conflict with the identity-var capture section
- [ ] **Rename completeness:** After renaming clawhub → skills, verify NO `.claude/skills/clawhub/` directories exist on agents from a fresh `rightclaw up` (old dirs must be cleaned up)
- [ ] **sandbox allows npx:** Fresh agent with default sandbox — verify `/skills search` returns results (not a silent network failure)
- [ ] **Policy gate format:** Install a skills.sh skill with known requirements — verify the gate checks CC sandbox settings, not just `metadata.openclaw` fields
- [ ] **Secret visibility:** Run `curl http://localhost:8080/process/info` — verify no secrets appear in the response
- [ ] **Backward compat:** Existing agents without `env:` in `agent.yaml` — verify `rightclaw up` succeeds without schema error
- [ ] **ClawHub source label:** Install a skill via ClawHub fallback — verify the output says "from ClawHub" not "from skills.sh"

## Recovery Strategies

| Pitfall | Recovery Cost | Recovery Steps |
|---------|---------------|----------------|
| Shell injection via env var | HIGH (security incident) | Rotate all secrets that were in env vars; audit wrapper scripts for injected code; fix quoting and regenerate wrappers |
| `installed.json` wiped by `rightclaw up` | MEDIUM | Re-install lost skills via `/skills install`; fix Pitfall 4 to prevent recurrence |
| `npx` blocked by sandbox | LOW | Add `registry.npmjs.org` and `github.com` to `agent.yaml` `sandbox.allowed_domains`, re-run `rightclaw up` |
| Old `clawhub/` dir present alongside new `skills/` dir | LOW | `rm -rf ~/.rightclaw/agents/<name>/.claude/skills/clawhub/`, restart agent |
| Env var ordering broke `ANTHROPIC_API_KEY` | LOW | Fix ordering in wrapper template, re-run `rightclaw up` to regenerate all wrappers |
| ClawHub fallback installed malicious skill | HIGH | `rm -rf .claude/skills/<name>/`, remove from `installed.json`, rotate any credentials the skill accessed |

## Pitfall-to-Phase Mapping

| Pitfall | Prevention Phase | Verification |
|---------|------------------|--------------|
| #1 Shell quoting | Phase 1: env var injection | `bash -n` on generated wrapper; test with `VAR="it's a 'test'"` |
| #2 HOME ordering | Phase 1: env var injection | Inspect generated wrapper; confirm env vars appear after HOME override block |
| #3 deny_unknown_fields migration | Phase 1: AgentConfig extension | Existing agents without `env:` load without error |
| #4 installed.json reset | Phase 1: install_builtin_skills fix | `rightclaw up` twice; `installed.json` unchanged after second run |
| #5 Policy gate format-agnostic | Phase 2: policy gate rework | Install a skills.sh skill; gate checks `allowedDomains` against skill docs |
| #6 Secret leakage to PC REST API | Phase 1: env var injection | `curl localhost:8080/process/info` shows no secrets |
| #7 npx blocked by sandbox | Phase 1 or 2: sandbox defaults | Fresh agent; `/skills search` returns results |
| #8 rename completeness | Phase 1: rename | `rg -l "clawhub"` returns only comments/docs after rename |
| #9 ClawHub fallback UX | Phase 2: ClawHub fallback design | Install output shows registry source; ClawHub requires explicit opt-in |
| #10 installed.json / on-disk divergence | Phase 1: same as #4 | `list` shows user-installed skills as "skills.sh" not "manual" after `rightclaw up` |

## Sources

### Codebase Audited
- `/home/wb/dev/rightclaw/templates/agent-wrapper.sh.j2` — current wrapper; identity vars captured before HOME override; no env var injection yet
- `/home/wb/dev/rightclaw/crates/rightclaw/src/codegen/shell_wrapper.rs` — `generate_wrapper()`; no shell escaping of user values
- `/home/wb/dev/rightclaw/crates/rightclaw/src/codegen/skills.rs` — `SKILL_CLAWHUB` const; `install_builtin_skills()` unconditionally writes `installed.json` as `{}`
- `/home/wb/dev/rightclaw/crates/rightclaw/src/agent/types.rs` — `AgentConfig` with `deny_unknown_fields`; no `env:` field yet
- `/home/wb/dev/rightclaw/crates/rightclaw/src/init.rs` — `clawhub` path references at lines 173, 250
- `/home/wb/dev/rightclaw/skills/clawhub/SKILL.md` — current `/skills` skill; policy gate checks `metadata.openclaw` only
- `/home/wb/dev/rightclaw/templates/process-compose.yaml.j2` — no `environment:` section; env vars handled in wrapper

### Project Context
- `.planning/seeds/SEED-005` — skills.sh background, registry landscape
- `.planning/seeds/SEED-006` — env var injection design, rename plan, policy gate rework
- `.planning/PROJECT.md` — v2.2 Active requirements

### Known Behavior (from v2.1 research)
- CC sandbox env var inheritance: env vars set before `exec` are inherited by the child process; bubblewrap does not strip env vars
- process-compose REST API exposes process config including env (confirmed via `process-compose` docs, v2.1 Pitfall 12)
- HOME override ordering contract: identity vars must be captured before HOME override (v2.1 Phase 8 design decision, now in wrapper)

---
*Pitfalls research for: v2.2 Skills Registry — registry replacement, env var injection, skill manager UX, CC sandbox interaction*
*Researched: 2026-03-25*
