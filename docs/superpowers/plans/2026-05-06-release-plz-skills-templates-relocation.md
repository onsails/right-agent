# Move skills/ and templates/ into right-agent crate (release-plz visibility)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make release-plz observe changes to bundled skills and prompt templates as `right-agent` crate changes, so prompt/skill edits trigger a release PR.

**Architecture:** Today `crates/right-agent/{skills,templates}` are symlinks pointing to workspace-root `skills/` and `templates/`. release-plz tracks file paths via git, not via symlink resolution, so changes recorded under `skills/...` and `templates/...` look like outside-the-crate changes. Variant A inverts the symlinks: physical content moves into `crates/right-agent/`, and the workspace root keeps backward-compat symlinks pointing into the crate. `include_dir!`/`include_str!` paths in source code already resolve crate-relative, so they keep working unchanged. One test that reads via `CARGO_MANIFEST_DIR/../../skills` is updated to crate-relative.

**Tech Stack:** git (rename detection), Cargo (`include_str!`/`include_dir!`), POSIX symlinks, release-plz.

---

### Task 1: Pre-flight verification

**Files:** none (read-only checks)

- [ ] **Step 1: Confirm clean working tree on `master`**

Run:

```bash
git status --short
git rev-parse --abbrev-ref HEAD
```

Expected:
- `git status --short` prints nothing.
- branch is `master`.

If the tree is dirty, stash or commit before proceeding. If branch is not `master`, the user must confirm (this plan modifies tracked file layouts).

- [ ] **Step 2: Confirm current symlink layout**

Run:

```bash
ls -la crates/right-agent/ | rg 'skills|templates'
ls -la skills templates 2>/dev/null | head -2
```

Expected:
- `crates/right-agent/skills` is a symlink → `../../skills`.
- `crates/right-agent/templates` is a symlink → `../../templates`.
- root `skills/` and `templates/` are real directories.

If anything else, STOP and report — the plan assumes this exact starting layout.

- [ ] **Step 3: Capture file count baseline**

Run:

```bash
git ls-files skills templates | wc -l
```

Expected: a positive number N. Record N for use in Task 8 verification.

---

### Task 2: Remove crate-side symlinks

**Files:**
- Delete: `crates/right-agent/skills` (symlink)
- Delete: `crates/right-agent/templates` (symlink)

- [ ] **Step 1: Remove the two symlinks via git**

Run:

```bash
git rm crates/right-agent/skills crates/right-agent/templates
```

Expected: git reports two files deleted. Working tree no longer has those entries.

- [ ] **Step 2: Verify removal**

Run:

```bash
ls crates/right-agent/skills crates/right-agent/templates 2>&1
git status --short
```

Expected:
- `ls` errors on both paths ("No such file or directory").
- `git status --short` shows two `D ` lines for the deleted symlinks (staged).

**Do NOT commit yet** — Task 3 produces the matching renames in the same commit.

---

### Task 3: Move root directories into the crate

**Files:**
- Rename: `skills/**` → `crates/right-agent/skills/**`
- Rename: `templates/**` → `crates/right-agent/templates/**`

- [ ] **Step 1: Move skills**

Run:

```bash
git mv skills crates/right-agent/skills
```

Expected: success, no output. All previously-tracked `skills/...` files are now at `crates/right-agent/skills/...` in the index.

- [ ] **Step 2: Move templates**

Run:

```bash
git mv templates crates/right-agent/templates
```

Expected: success, no output.

- [ ] **Step 3: Verify rename detection**

Run:

```bash
git status --short | head -10
git diff --cached --diff-filter=R --name-status | head -10
```

Expected:
- `git status --short` shows `R ` (rename) entries with paths `skills/X -> crates/right-agent/skills/X` and `templates/Y -> crates/right-agent/templates/Y`.
- The R-filter diff lists those renames explicitly.

If git shows `D ` + `A ` instead of `R ` for some files (rename detection threshold), that is acceptable — the file is still tracked at the new path. Do not chase 100% rename markers.

- [ ] **Step 4: Verify file count is unchanged**

Run:

```bash
git ls-files crates/right-agent/skills crates/right-agent/templates | wc -l
```

Expected: equals N from Task 1 Step 3.

If the count is lower, some file did not move — investigate before continuing.

---

### Task 4: Create root symlinks pointing into the crate

**Files:**
- Create: `skills` (symlink → `crates/right-agent/skills`)
- Create: `templates` (symlink → `crates/right-agent/templates`)

- [ ] **Step 1: Create the two symlinks**

Run:

```bash
ln -s crates/right-agent/skills skills
ln -s crates/right-agent/templates templates
```

Expected: success, no output.

- [ ] **Step 2: Verify symlinks resolve**

Run:

```bash
ls -la skills templates
test -f skills/rightmcp/SKILL.md && echo OK1
test -f templates/right/prompt/OPERATING_INSTRUCTIONS.md && echo OK2
```

Expected:
- `ls -la` prints two `l...` entries with targets `crates/right-agent/skills` and `crates/right-agent/templates`.
- Both `OK1` and `OK2` print (the symlinks resolve to real files).

- [ ] **Step 3: Stage the new symlinks**

Run:

```bash
git add skills templates
git status --short | rg '^A.*(skills|templates)$'
```

Expected: two `A ` lines for `skills` and `templates` (the new symlink files).

Verify git is storing them as symlinks:

```bash
git ls-files --stage skills templates
```

Expected: both lines start with mode `120000` (git's symlink mode), not `100644`.

---

### Task 5: Update the one source-code reference that escaped the crate

**Files:**
- Modify: `crates/right-agent/src/codegen/skills.rs` (around line 281)

- [ ] **Step 1: Locate the line**

Run:

```bash
rg -n '\.join\("\.\./\.\./skills"\)' crates/right-agent/src/codegen/skills.rs
```

Expected: one hit on line 281 (or near):

```
let source_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("../../skills")
    .join(source_name);
```

- [ ] **Step 2: Replace `../../skills` with `skills`**

Edit `crates/right-agent/src/codegen/skills.rs`:

Change from:

```rust
            let source_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../skills")
                .join(source_name);
```

To:

```rust
            let source_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("skills")
                .join(source_name);
```

Why: `CARGO_MANIFEST_DIR` is `crates/right-agent/`. After Task 3 the real skills dir lives at `crates/right-agent/skills/`. The old `../../skills` path used to land on the workspace-root real dir; after the move that root path is a symlink, and following symlinks during a test walk is unnecessarily indirect. Use the crate-relative path.

- [ ] **Step 3: Verify no other `../../skills` or `../../templates` remain in source**

Run:

```bash
rg -n '\.\./\.\./skills|\.\./\.\./templates' crates/
```

Expected: zero hits (the `include_str!`/`include_dir!` macros in the codebase use `../templates`, `../../templates`, and `$CARGO_MANIFEST_DIR/skills/...`, all of which already resolve crate-relative).

If any other hit appears, replace it with the crate-relative form analogously.

---

### Task 6: Build verification

**Files:** none (compile check)

- [ ] **Step 1: Build the workspace in debug**

Run:

```bash
devenv shell -- cargo build --workspace
```

Expected: `Finished` line, zero errors.

The macros to watch are:
- `include_str!("../templates/right/agent/...")` in `crates/right-agent/src/init.rs`
- `include_str!("../../templates/...")` in `crates/right-agent/src/codegen/{agent_def.rs,cloudflared.rs,process_compose.rs}`
- `include_dir!("$CARGO_MANIFEST_DIR/skills/...")` in `crates/right-agent/src/codegen/skills.rs`

All resolve to `crates/right-agent/templates/...` and `crates/right-agent/skills/...` after the move — the same physical files they used to read through the symlinks.

If a macro fails with "file not found", the file was missed by `git mv`. Run `git ls-files crates/right-agent/skills crates/right-agent/templates | rg <missing-name>` to confirm it landed in the new tree, and re-run.

---

### Task 7: Test verification

**Files:** none (run existing tests)

- [ ] **Step 1: Run the codegen::skills test that walks the skill source tree**

Run:

```bash
devenv shell -- cargo test -p right-agent --lib codegen::skills -- --nocapture
```

Expected: all tests in that module pass. Specifically the test edited in Task 5 (`install_builtin_skills` walker) reads `CARGO_MANIFEST_DIR/skills/...` and asserts each file installed.

- [ ] **Step 2: Run the workspace test suite**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: zero failures. If a sandbox integration test fails for unrelated reasons (OpenShell not running, network), confirm with the user before treating it as a blocker.

---

### Task 8: Commit

**Files:** none (git only)

- [ ] **Step 1: Final review of staged changes**

Run:

```bash
git status --short | head -20
git diff --cached --stat | tail -10
```

Expected:
- Two `D ` for the removed crate-side symlinks (Task 2).
- Many `R ` (or `D`+`A`) for renamed files under `skills/` and `templates/` (Task 3).
- Two `A ` for the new root symlinks (Task 4).
- One `M ` for `crates/right-agent/src/codegen/skills.rs` (Task 5).

`--stat` should show net file count near zero (renames balance) plus a tiny diff in `skills.rs`.

- [ ] **Step 2: Commit**

Run:

```bash
git commit -m "$(cat <<'EOF'
refactor: move skills/ and templates/ into right-agent crate

Physical content now lives in crates/right-agent/{skills,templates};
workspace-root skills and templates are symlinks pointing back into
the crate. release-plz tracks files by git path and treated previous
edits to root skills/templates as outside-crate noise, suppressing
release-pr creation. With the content under crates/right-agent/, the
right-agent package is correctly considered changed and release-plz
will produce release PRs for prompt and skill edits.

include_str!/include_dir! macros are unchanged: they already used
crate-relative paths that resolved through the symlinks. One test
walker in codegen::skills updated from ../../skills to crate-relative
skills/.
EOF
)"
```

Expected: commit succeeds. Pre-commit hooks (if any) pass.

If a hook fails: fix the issue, re-stage, create a NEW commit (do not `--amend`).

- [ ] **Step 3: Verify the commit lands cleanly**

Run:

```bash
git log -1 --stat | head -20
git status --short
```

Expected:
- Latest commit is the refactor.
- Working tree is clean.

---

### Task 9: Post-merge verification (informational, not part of this plan's commit)

**This task is not executed as part of the implementation. It documents what to look for after the commit reaches `master` and CI runs.**

Once this commit is on `master`:

1. The next push to `master` triggers `.github/workflows/release-plz.yml`.
2. The `Release-plz PR` job should now log a non-empty `release_pr_output` containing a PR entry — because the renamed files inside `crates/right-agent/` count as package changes since the `right-agent-v0.2.9` tag.
3. release-plz will open a PR bumping the workspace version (the three packages share `version_group = "workspace"`).

If the PR still does not appear:
- Open the workflow run and re-read `release_pr_output:` in the `Release-plz PR` job log.
- Run `git log --name-only right-agent-v0.2.9..HEAD -- crates/right-agent` locally to confirm release-plz sees changes.
- Check release-plz.toml — workspace flags `dependencies_update = false` and `changelog_update = false` are intentional and do not block a release PR; they only suppress changelog churn from cross-package dep bumps.
