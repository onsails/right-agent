# Dashboard `build.rs`-driven frontend pipeline — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move dashboard frontend compilation into Cargo's build phase so the embedded bundle can never drift from source, and stop tracking `crates/right-dashboard/static/` in git (including Git LFS).

**Architecture:** Add `crates/right-dashboard/build.rs` that invokes `vite build` with `outDir=$OUT_DIR/dashboard`. Switch `assets.rs` to `include_dir!("$OUT_DIR/dashboard")`. Vite's outDir becomes env-driven (with a fallback for manual `npm run build`). The pre-existing `static/dashboard/` checked-in bundle (including LFS-tracked JS/CSS) is deleted and gitignored. CI release workflow gains an `actions/setup-node` step; CI tests workflow inherits node from devenv shell.

**Tech Stack:** Rust (Cargo build script), Vite, Vue, npm, `include_dir` crate, GitHub Actions (`actions/setup-node@v4`).

**Spec:** `docs/superpowers/specs/2026-05-28-dashboard-buildrs-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `crates/right-dashboard/build.rs` | **create** | Cargo build script: verifies node/npm, runs `npm install` if needed, invokes `npx vite build` with `VITE_OUT_DIR=$OUT_DIR/dashboard`. |
| `crates/right-dashboard/frontend/vite.config.ts` | modify | Read `outDir` from `process.env.VITE_OUT_DIR` (fallback `'../static/dashboard'`). |
| `crates/right-dashboard/src/assets.rs` | modify | One-line change: `include_dir!("$OUT_DIR/dashboard")`. Plus new `#[cfg(test)]` regression test that asserts `ProvidersView` appears in the embedded bundle. |
| `crates/right-dashboard/static/dashboard/**` | **delete** (git rm) | Stale checked-in build artifact. |
| `.gitattributes` | modify | Remove the two LFS / linguist-generated lines for `static/dashboard/`. |
| `.gitignore` | modify | Add `crates/right-dashboard/static/`. |
| `.github/workflows/build.yml` | modify | Add `actions/setup-node@v4` (with npm cache) before the cargo build step. |
| `.github/workflows/tests.yml` | modify | Add a step that runs the new `dashboard_bundle_contains_providers_view` test explicitly. |

---

## Verification cadence (per project AGENTS.md)

- Baseline (Task 1): one targeted check — `devenv shell -- cargo test -p right-dashboard`.
- Mid-flight: after each task touches Rust, run `devenv shell -- cargo test -p right-dashboard` (narrow). Workspace tests are NOT required between tasks.
- Final (Task 9): mandatory `devenv shell -- cargo test --workspace` and `devenv shell -- cargo build --workspace`.

---

## Task 1: Baseline verification

**Files:** none (read-only)

- [ ] **Step 1: Confirm we're in the right worktree on the right branch**

```bash
pwd
git rev-parse --abbrev-ref HEAD
git status
```

Expected: working directory `/Users/molt/dev/rightclaw/.worktrees/providers`, branch `feat/providers`, working tree clean.

- [ ] **Step 2: Capture baseline for right-dashboard**

```bash
devenv shell -- cargo test -p right-dashboard 2>&1 | tail -20
```

Expected: tests pass (record pass count; any pre-existing failures must be noted before proceeding).

- [ ] **Step 3: Confirm vite build still works from inside frontend/ (sanity)**

```bash
cd crates/right-dashboard/frontend
ls node_modules/ >/dev/null 2>&1 && echo "node_modules present" || echo "node_modules MISSING (npm install will be exercised in Task 3)"
cd -
```

No assertion — informational only.

---

## Task 2: Make `vite.config.ts` read `outDir` from env

**Files:**
- Modify: `crates/right-dashboard/frontend/vite.config.ts`

- [ ] **Step 1: Read current vite.config.ts**

```bash
cat crates/right-dashboard/frontend/vite.config.ts
```

Expected content:
```ts
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

export default defineConfig({
  base: './',
  plugins: [vue()],
  build: {
    emptyOutDir: true,
    outDir: '../static/dashboard',
    assetsDir: 'generated/assets',
    sourcemap: false,
  },
})
```

- [ ] **Step 2: Edit `outDir` to honor `VITE_OUT_DIR`**

Change `outDir: '../static/dashboard'` to `outDir: process.env.VITE_OUT_DIR ?? '../static/dashboard'`.

Final file content:

```ts
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

const outDir = process.env.VITE_OUT_DIR ?? '../static/dashboard'

export default defineConfig({
  base: './',
  plugins: [vue()],
  build: {
    emptyOutDir: true,
    outDir,
    assetsDir: 'generated/assets',
    sourcemap: false,
  },
})
```

- [ ] **Step 3: Verify manual build still works (fallback path)**

```bash
cd crates/right-dashboard/frontend
unset VITE_OUT_DIR
devenv shell -- npm run build 2>&1 | tail -5
ls ../static/dashboard/index.html
cd -
```

Expected: `index.html` exists at `crates/right-dashboard/static/dashboard/index.html`. This regenerates the (still-tracked) bundle from current source — that's intentional; we still want the bundle present until Task 5 removes it.

- [ ] **Step 4: Verify env override works**

```bash
cd crates/right-dashboard/frontend
TMP=$(mktemp -d)
VITE_OUT_DIR="$TMP/dashboard" devenv shell -- npx vite build 2>&1 | tail -5
ls "$TMP/dashboard/index.html"
rm -rf "$TMP"
cd -
```

Expected: `index.html` exists at `$TMP/dashboard/index.html`. Vite honored the env var.

- [ ] **Step 5: Commit**

```bash
git add crates/right-dashboard/frontend/vite.config.ts crates/right-dashboard/static/
git commit -m "build(dashboard): read outDir from VITE_OUT_DIR with fallback"
```

(The Task 2 step-3 npm build regenerated the static/ bundle; commit it alongside so the working tree is clean. Task 5 will rm it.)

---

## Task 3: Add `build.rs` that runs vite into `$OUT_DIR/dashboard`

**Files:**
- Create: `crates/right-dashboard/build.rs`

- [ ] **Step 1: Create `crates/right-dashboard/build.rs`**

```rust
use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"),
    );
    let frontend = manifest_dir.join("frontend");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR not set"));
    let dashboard_out = out_dir.join("dashboard");

    // Tell cargo which sources to watch
    for path in [
        "frontend/src",
        "frontend/index.html",
        "frontend/vite.config.ts",
        "frontend/tsconfig.json",
        "frontend/package.json",
        "frontend/package-lock.json",
        "build.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    require_tool("node");
    require_tool("npm");

    if !frontend.join("node_modules").exists() {
        let mut npm = Command::new("npm");
        npm.args(["install"]).current_dir(&frontend);
        run(&mut npm);
    }

    let mut vite = Command::new("npx");
    vite.args(["vite", "build"])
        .current_dir(&frontend)
        .env("VITE_OUT_DIR", &dashboard_out);
    run(&mut vite);

    let index = dashboard_out.join("index.html");
    assert!(
        index.exists(),
        "vite build completed but {} not found",
        index.display(),
    );
}

fn require_tool(name: &str) {
    if which(name).is_none() {
        eprintln!(
            "error: '{name}' not found on PATH. Enter the devenv shell ('devenv shell') or install Node.js (>= 20).",
        );
        std::process::exit(1);
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn run(cmd: &mut Command) {
    let program = cmd.get_program().to_owned();
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to spawn {program:?}: {e}"));
    assert!(
        status.success(),
        "{program:?} exited with {status}",
    );
}
```

- [ ] **Step 2: Build right-dashboard to trigger build.rs**

```bash
devenv shell -- cargo build -p right-dashboard 2>&1 | tail -10
```

Expected: build succeeds. The build script runs vite which prints its own banner.

- [ ] **Step 3: Verify `$OUT_DIR/dashboard/index.html` was produced**

```bash
find target/devenv/debug/build -name 'right-dashboard-*' -type d 2>/dev/null \
  | head -1 \
  | xargs -I{} ls {}/out/dashboard/
```

Expected: lists `index.html`, `generated/` directory. (Path naming follows Cargo convention: `target/<profile>/build/<crate>-<hash>/out/dashboard/`.)

- [ ] **Step 4: Verify the produced bundle contains `ProvidersView`**

```bash
find target/devenv/debug/build -name 'right-dashboard-*' -type d 2>/dev/null \
  | head -1 \
  | xargs -I{} grep -l ProvidersView {}/out/dashboard/generated/assets/*.js
```

Expected: at least one JS file matches. This proves vite picked up the current frontend source.

- [ ] **Step 5: Verify no rebuild on no-op (cargo cache works)**

```bash
devenv shell -- cargo build -p right-dashboard 2>&1 | tail -5
```

Expected: `Finished ... target(s) in <small> s` — no recompilation, build script does NOT re-run.

- [ ] **Step 6: Verify rebuild on frontend source change**

```bash
touch crates/right-dashboard/frontend/src/views/ProvidersView.vue
devenv shell -- cargo build -p right-dashboard 2>&1 | tail -10
```

Expected: build script re-runs (vite banner appears again). Restore mtime semantics by leaving the touched file as-is (no reset needed; content unchanged).

- [ ] **Step 7: Commit**

```bash
git add crates/right-dashboard/build.rs
git commit -m "build(dashboard): add build.rs that runs vite into OUT_DIR"
```

---

## Task 4: Switch `assets.rs` to `$OUT_DIR/dashboard` (TDD)

**Files:**
- Modify: `crates/right-dashboard/src/assets.rs`

This task uses TDD: write a failing test first (the bundle is stale because assets.rs still points at `static/`), then change the `include_dir!` argument, verify the test passes.

- [ ] **Step 1: Add a failing regression test to `assets.rs`**

Append to `crates/right-dashboard/src/assets.rs`:

```rust

#[cfg(test)]
mod tests {
    use super::DASHBOARD_ASSETS;
    use include_dir::{Dir, DirEntry};

    fn contains_providers_view(dir: &Dir<'_>) -> bool {
        for entry in dir.entries() {
            match entry {
                DirEntry::File(f) => {
                    let path = f.path();
                    let is_js_or_html = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e == "js" || e == "html")
                        .unwrap_or(false);
                    if is_js_or_html {
                        if let Ok(s) = std::str::from_utf8(f.contents()) {
                            if s.contains("ProvidersView") {
                                return true;
                            }
                        }
                    }
                }
                DirEntry::Dir(d) => {
                    if contains_providers_view(d) {
                        return true;
                    }
                }
            }
        }
        false
    }

    #[test]
    fn dashboard_bundle_contains_providers_view() {
        assert!(
            contains_providers_view(&DASHBOARD_ASSETS),
            "DASHBOARD_ASSETS has no JS/HTML file containing 'ProvidersView' \
             — bundle is stale (vite build did not run for current source)",
        );
    }
}
```

- [ ] **Step 2: Run the test and watch it FAIL**

```bash
devenv shell -- cargo test -p right-dashboard dashboard_bundle_contains_providers_view 2>&1 | tail -20
```

Expected: **FAIL** with the assertion message. The bundle embedded today comes from `static/dashboard/` which is stale (predates Providers feature).

> If this step PASSES instead of failing, it means the npm build in Task 2 step 3 regenerated `static/dashboard/` and that fresh bundle DOES include `ProvidersView`. That's still a valid red→green only if you first revert `static/dashboard/` to its pre-Task-2 state. To force the red state cleanly: `git checkout HEAD~1 -- crates/right-dashboard/static/dashboard/` and re-run. After that, proceed and re-apply Step 3 below.

- [ ] **Step 3: Change `include_dir!` argument to `$OUT_DIR/dashboard`**

In `crates/right-dashboard/src/assets.rs`, line 3:

```rust
- static DASHBOARD_ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static/dashboard");
+ static DASHBOARD_ASSETS: Dir<'_> = include_dir!("$OUT_DIR/dashboard");
```

- [ ] **Step 4: Run the test and watch it PASS**

```bash
devenv shell -- cargo test -p right-dashboard dashboard_bundle_contains_providers_view 2>&1 | tail -20
```

Expected: **PASS**. The bundle now comes from `$OUT_DIR/dashboard` which was freshly produced by `build.rs` in Task 3.

- [ ] **Step 5: Run full right-dashboard test suite**

```bash
devenv shell -- cargo test -p right-dashboard 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/right-dashboard/src/assets.rs
git commit -m "build(dashboard): embed bundle from OUT_DIR + regression test"
```

---

## Task 5: Untrack `static/` from git and LFS

**Files:**
- Delete (git rm): `crates/right-dashboard/static/dashboard/index.html`
- Delete (git rm): `crates/right-dashboard/static/dashboard/generated/assets/charts-C0wBuzb3.js`
- Delete (git rm): `crates/right-dashboard/static/dashboard/generated/assets/dist-CPrJONjh.js`
- Delete (git rm): `crates/right-dashboard/static/dashboard/generated/assets/echarts-BNmedilC.js`
- Delete (git rm): `crates/right-dashboard/static/dashboard/generated/assets/index-BXa5TH-A.css`
- Delete (git rm): `crates/right-dashboard/static/dashboard/generated/assets/index-CFcc0da6.js`
- Modify: `.gitattributes`
- Modify: `.gitignore`

> NOTE: file names in `generated/assets/` contain content hashes. If
> Task 2's npm build produced different hashed filenames, the `git rm`
> below will fail with "did not match any files." In that case, list
> the current files first via `git ls-files crates/right-dashboard/static/`
> and `git rm` each one. The principle holds: remove EVERY tracked path
> under `static/dashboard/`.

- [ ] **Step 1: List all tracked files under static/**

```bash
git ls-files crates/right-dashboard/static/
```

Record the output. These are the paths to remove.

- [ ] **Step 2: git rm every tracked file under static/**

```bash
git rm -r crates/right-dashboard/static/
```

(Bulk recursive form. `git rm -r` on a tracked directory is safe.)

Expected: each file printed with `rm '...'`. Working tree no longer has these files.

- [ ] **Step 3: Remove LFS `.gitattributes` lines**

Edit `.gitattributes`. Delete these two lines:

```
crates/right-dashboard/static/dashboard/** linguist-generated=true
crates/right-dashboard/static/dashboard/generated/assets/** filter=lfs diff=lfs merge=lfs -text linguist-generated=true
```

Verify:

```bash
grep "right-dashboard/static" .gitattributes
```

Expected: no matches.

- [ ] **Step 4: Add `static/` to `.gitignore`**

Append to `.gitignore`:

```
# Dashboard frontend artifacts — generated by build.rs into $OUT_DIR
# and (on manual `npm run build`) into the fallback static/ path.
crates/right-dashboard/static/
```

(Do NOT add `crates/right-dashboard/frontend/node_modules/` here if a
top-level `node_modules/` ignore rule already covers it.)

Check whether node_modules is already covered:

```bash
git check-ignore crates/right-dashboard/frontend/node_modules
```

If the command outputs the path, it's already ignored — skip. Otherwise add `node_modules/` to `.gitignore` too.

- [ ] **Step 5: Verify static/ is now ignored AND a fresh build still works**

```bash
git check-ignore crates/right-dashboard/static/dashboard/index.html
```

Expected: the path is printed (meaning ignored).

```bash
devenv shell -- cargo build -p right-dashboard 2>&1 | tail -5
devenv shell -- cargo test -p right-dashboard dashboard_bundle_contains_providers_view 2>&1 | tail -5
```

Expected: build OK, test passes. (The bundle now comes only from $OUT_DIR; static/ may not even exist as a directory.)

- [ ] **Step 6: Commit**

```bash
git add .gitattributes .gitignore
git commit -m "build(dashboard): untrack static/ (now built into OUT_DIR by build.rs)"
```

(`git rm` already staged the deletions in Step 2.)

---

## Task 6: Add `setup-node` to release build workflow

**Files:**
- Modify: `.github/workflows/build.yml`

- [ ] **Step 1: Read current build.yml**

```bash
cat .github/workflows/build.yml
```

Identify the step block: `Install Rust toolchain` (a `dtolnay/rust-toolchain@stable` action). The new step must run BEFORE `Build`.

- [ ] **Step 2: Insert `actions/setup-node@v4` step**

In `.github/workflows/build.yml`, insert between the `Install Rust toolchain` step and the `Build` step:

```yaml
      - name: Install Node.js
        uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'
          cache-dependency-path: crates/right-dashboard/frontend/package-lock.json
```

(Indentation: 6 spaces for the `- name:` line, matching surrounding steps.)

- [ ] **Step 3: Verify YAML parses (no in-tree CI to run, validate locally)**

```bash
devenv shell -- python3 -c "import yaml; yaml.safe_load(open('.github/workflows/build.yml'))" \
  && echo "YAML valid"
```

Expected: `YAML valid`. (If `python3` lacks `yaml`, fall back to any YAML linter; the key check is parse success.)

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "ci(build): install Node.js for cargo build in release workflow"
```

---

## Task 7: Add bundle-content regression step to tests workflow

**Files:**
- Modify: `.github/workflows/tests.yml`

The test added in Task 4 (`dashboard_bundle_contains_providers_view`) runs as part of `cargo test --workspace`. This task simply ensures the existing workspace-test step covers it (no new step needed) AND adds an explicit guard so the failure mode is obvious in CI logs.

- [ ] **Step 1: Confirm the existing workspace step covers the new test**

```bash
grep -A2 "cargo test --workspace" .github/workflows/tests.yml
```

Expected: there is a step running `devenv shell -- cargo test --workspace` (or similar). The new test will run as part of it.

- [ ] **Step 2: Add an explicit `dashboard bundle` step BEFORE the workspace test**

In `.github/workflows/tests.yml`, find the existing `cargo test --workspace` step. Immediately BEFORE it, add:

```yaml
      - name: Dashboard bundle freshness
        run: devenv shell -- cargo test -p right-dashboard dashboard_bundle_contains_providers_view
```

Rationale: a targeted failure here makes the "frontend not rebuilt" failure mode unambiguous in CI logs, before the full workspace test runs.

- [ ] **Step 3: Verify YAML parses**

```bash
devenv shell -- python3 -c "import yaml; yaml.safe_load(open('.github/workflows/tests.yml'))" \
  && echo "YAML valid"
```

Expected: `YAML valid`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/tests.yml
git commit -m "ci(tests): add explicit dashboard-bundle freshness step"
```

---

## Task 8: Cold-checkout smoke test (manual)

**Files:** none (verification only)

- [ ] **Step 1: Create a fresh checkout in a tempdir**

```bash
TMPCO=$(mktemp -d)
git clone --branch feat/providers /Users/molt/dev/rightclaw/.worktrees/providers "$TMPCO/repo"
cd "$TMPCO/repo"
```

- [ ] **Step 2: Build the workspace from cold**

```bash
devenv shell -- cargo build -p right-dashboard 2>&1 | tail -15
```

Expected: build succeeds. The first build runs `npm install` (slow — ~30-90s) then `vite build`. No prebuilt assets in tree; all generated.

- [ ] **Step 3: Run the bundle freshness test from cold**

```bash
devenv shell -- cargo test -p right-dashboard dashboard_bundle_contains_providers_view 2>&1 | tail -5
```

Expected: test passes.

- [ ] **Step 4: Tear down**

```bash
cd /Users/molt/dev/rightclaw/.worktrees/providers
rm -rf "$TMPCO"
```

No commit — verification only.

---

## Task 9: Final full workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Full workspace build**

```bash
devenv shell -- cargo build --workspace 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 2: Full workspace test (mandatory per AGENTS.md)**

```bash
devenv shell -- cargo test --workspace 2>&1 | grep -E "^test result:" | tail -60
```

Expected: every line ends in `0 failed`. Record total pass count.

- [ ] **Step 3: Inspect commit log of this plan**

```bash
git log --oneline feat/providers ^master | head -20
```

Expected: commits from Tasks 2, 3, 4, 5, 6, 7 visible (6 new commits on top of the prior providers work).

- [ ] **Step 4: Done**

No commit. The plan's deliverable is the set of commits produced by Tasks 2–7 plus a green workspace test.

---

## Self-review notes

- **Spec coverage check:** every spec section has a task — vite.config.ts (Task 2), build.rs (Task 3), assets.rs change + regression test (Task 4), `git rm` static/ + `.gitattributes` + `.gitignore` (Task 5), build.yml setup-node (Task 6), tests.yml explicit step (Task 7), cold-checkout smoke (Task 8), final workspace test (Task 9).
- **Placeholders:** none — every step has a concrete command or code block.
- **Type consistency:** the regression-test function name `dashboard_bundle_contains_providers_view` matches between Task 4 (definition) and Task 7 (CI step that runs it).
- **Spec non-goals respected:** no `vue-tsc` step inside build.rs; no LFS history rewrite; no hot-reload; no separate typecheck workflow (deferred).
- **Hard-fail-on-missing-node** is implemented in `require_tool()` inside the Task 3 build.rs body and tested implicitly (build will fail loudly outside devenv shell).
