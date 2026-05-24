# Dashboard Generated Assets Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make committed dashboard static assets visibly generated, store generated chunks in Git LFS, and preserve the existing embedded dashboard serving model.

**Architecture:** Keep `crates/right-dashboard/static/dashboard/index.html` at the static root so existing `/dashboard/<agent>/` routing and `include_dir!` embedding remain unchanged. Configure Vite to emit hashed JS/CSS chunks under `static/dashboard/generated/assets/`, mark all checked-in dashboard static output as generated for GitHub Linguist, and store only generated chunks in Git LFS. CI and release checkouts must fetch LFS objects before Rust compilation so `include_dir!` embeds real asset bytes instead of LFS pointer text.

**Tech Stack:** Vite, Vue, Git attributes, Git LFS, GitHub Actions, Rust `include_dir`, Axum static dashboard route.

---

## File Structure

- Create: `.gitattributes`
  - Owns repository-level GitHub Linguist and Git LFS path attributes.
- Modify: `devenv.nix`
  - Provides `git-lfs` in the shared development shell.
- Modify: `crates/right-dashboard/frontend/vite.config.ts`
  - Owns dashboard frontend build output layout.
- Modify generated output under: `crates/right-dashboard/static/dashboard/`
  - `index.html` stays at the static root and is marked generated, but is not stored in LFS.
  - Vite hashed JS/CSS chunks move to `generated/assets/`, are marked generated, and are stored in LFS.
- Modify: `.github/workflows/build.yml`
  - Ensures release binary builds fetch Git LFS asset bytes.
- Modify: `.github/workflows/tests.yml`
  - Ensures Rust test jobs fetch Git LFS asset bytes.
- Modify: `.github/workflows/release-plz.yml`
  - Ensures release-plz package and release jobs fetch Git LFS asset bytes.
- Modify: `docs/architecture/modules.md`
  - Documents that dashboard static output is checked-in generated content and that hashed chunks are LFS-backed.

## Task 1: Add Git LFS And Generated Attributes

**Files:**
- Create: `.gitattributes`
- Modify: `devenv.nix`

- [ ] **Step 1: Add Git LFS to the devenv package set**

In `devenv.nix`, add `git-lfs` to the `packages = with pkgs; [` list:

```nix
  packages = with pkgs; [
    process-compose
    socat
    ripgrep          # SBOX-04: CC sandbox rg check; must be in agent launch PATH
    grpcurl
    protobuf
    ffmpeg
    cmake            # required by whisper-rs-sys build script
    sccache
    actionlint
    nodejs
    git-lfs
  ] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.bubblewrap
  ];
```

- [ ] **Step 2: Verify current attributes are absent**

Run:

```bash
devenv shell -- git check-attr linguist-generated -- crates/right-dashboard/static/dashboard/index.html
devenv shell -- git check-attr filter -- crates/right-dashboard/static/dashboard/generated/assets/example.js
```

Expected: the first output ends with `linguist-generated: unspecified`; the second output ends with `filter: unspecified`.

- [ ] **Step 3: Create `.gitattributes`**

Add this file at `.gitattributes`:

```gitattributes
crates/right-dashboard/static/dashboard/** linguist-generated=true
crates/right-dashboard/static/dashboard/generated/assets/** filter=lfs diff=lfs merge=lfs -text linguist-generated=true
```

- [ ] **Step 4: Verify attributes apply to root dashboard HTML without LFS**

Run:

```bash
devenv shell -- git check-attr linguist-generated -- crates/right-dashboard/static/dashboard/index.html
devenv shell -- git check-attr filter -- crates/right-dashboard/static/dashboard/index.html
```

Expected: the first output ends with `linguist-generated: set`; the second output ends with `filter: unspecified`.

- [ ] **Step 5: Verify attributes apply to generated nested assets with LFS**

Run:

```bash
devenv shell -- git check-attr linguist-generated -- crates/right-dashboard/static/dashboard/generated/assets/example.js
devenv shell -- git check-attr filter -- crates/right-dashboard/static/dashboard/generated/assets/example.js
```

Expected: the first output ends with `linguist-generated: set`; the second output ends with `filter: lfs`.

## Task 2: Move Vite Asset Output Under `generated/assets`

**Files:**
- Modify: `crates/right-dashboard/frontend/vite.config.ts`
- Modify generated output under: `crates/right-dashboard/static/dashboard/`

- [ ] **Step 1: Update Vite build output config**

Replace `crates/right-dashboard/frontend/vite.config.ts` with:

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

- [ ] **Step 2: Rebuild dashboard static output**

Run:

```bash
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: Vite reports a successful production build and output paths under `../static/dashboard/generated/assets/`.

- [ ] **Step 3: Verify generated asset directory exists**

Run:

```bash
devenv shell -- test -d crates/right-dashboard/static/dashboard/generated/assets
```

Expected: exit code 0.

- [ ] **Step 4: Verify old asset directory is gone**

Run:

```bash
devenv shell -- test ! -d crates/right-dashboard/static/dashboard/assets
```

Expected: exit code 0.

- [ ] **Step 5: Verify generated `index.html` references the new directory**

Run:

```bash
devenv shell -- rg -n "generated/assets" crates/right-dashboard/static/dashboard/index.html
```

Expected: at least one match for JS and one match for CSS.

- [ ] **Step 6: Stage regenerated assets through Git LFS**

Run:

```bash
devenv shell -- git lfs install --local
devenv shell -- git add -- crates/right-dashboard/static/dashboard
devenv shell -- git lfs ls-files crates/right-dashboard/static/dashboard/generated/assets
```

Expected: `git lfs ls-files` lists the generated JS and CSS chunk files under `generated/assets/`.

- [ ] **Step 7: Verify generated assets are not LFS pointer text in the working tree**

Run:

```bash
devenv shell -- bash -lc '! rg -n "^version https://git-lfs.github.com/spec/v1" crates/right-dashboard/static/dashboard/generated/assets'
```

Expected: exit code 0.

## Task 3: Fetch LFS Assets In CI And Release Workflows

**Files:**
- Modify: `.github/workflows/build.yml`
- Modify: `.github/workflows/tests.yml`
- Modify: `.github/workflows/release-plz.yml`

- [ ] **Step 1: Update release binary checkout**

In `.github/workflows/build.yml`, change the checkout step to:

```yaml
      - name: Checkout repository
        uses: actions/checkout@v6
        with:
          lfs: true
```

- [ ] **Step 2: Update default test checkout**

In `.github/workflows/tests.yml`, change the first checkout step to:

```yaml
      - uses: actions/checkout@v6
        with:
          lfs: true
```

- [ ] **Step 3: Update ignored-test checkout**

In `.github/workflows/tests.yml`, change the second checkout step to:

```yaml
      - uses: actions/checkout@v6
        with:
          lfs: true
```

- [ ] **Step 4: Update release-plz checkouts**

In `.github/workflows/release-plz.yml`, change both checkout steps to:

```yaml
      - uses: actions/checkout@v6
        with:
          lfs: true
```

- [ ] **Step 5: Verify relevant checkout steps request LFS**

Run:

```bash
devenv shell -- rg -n "lfs: true" .github/workflows/build.yml .github/workflows/tests.yml .github/workflows/release-plz.yml
```

Expected: five matches: one in `build.yml`, two in `tests.yml`, and two in `release-plz.yml`.

## Task 4: Update Architecture Documentation

**Files:**
- Modify: `docs/architecture/modules.md`

- [ ] **Step 1: Re-read dashboard module docs**

Run:

```bash
devenv shell -- sed -n '71,88p' docs/architecture/modules.md
```

Expected: current dashboard module list includes `frontend/` and `static/dashboard/`.

- [ ] **Step 2: Update the static dashboard bullet**

In `docs/architecture/modules.md`, replace the current static dashboard bullet with:

```markdown
- `static/dashboard/` — checked-in generated dashboard output embedded into the Rust binary; Vite hashed chunks live under `generated/assets/` and are stored in Git LFS.
```

- [ ] **Step 3: Confirm `ARCHITECTURE.md` does not need a change**

Run:

```bash
devenv shell -- rg -n "right-dashboard|dashboard API|route|static" ARCHITECTURE.md
```

Expected: existing architecture text still describes the same crate boundary and route contract. Do not edit `ARCHITECTURE.md` unless the implementation changes the route contract or crate ownership.

## Task 5: Verify And Commit

**Files:**
- Stage only `.gitattributes`, `devenv.nix`, `.github/workflows/build.yml`, `.github/workflows/tests.yml`, `.github/workflows/release-plz.yml`, `crates/right-dashboard/frontend/vite.config.ts`, `crates/right-dashboard/static/dashboard/`, and `docs/architecture/modules.md`.

- [ ] **Step 1: Verify Git LFS is available in devenv**

Run:

```bash
devenv shell -- git lfs version
```

Expected: exit code 0 and a Git LFS version string.

- [ ] **Step 2: Verify generated assets are tracked by LFS**

Run:

```bash
devenv shell -- git lfs ls-files crates/right-dashboard/static/dashboard/generated/assets
```

Expected: generated JS and CSS chunk files are listed.

- [ ] **Step 3: Run frontend regression tests**

Run:

```bash
devenv shell -- npm test --prefix crates/right-dashboard/frontend
```

Expected: all dashboard frontend tests pass.

- [ ] **Step 4: Run frontend typecheck**

Run:

```bash
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
```

Expected: exit code 0.

- [ ] **Step 5: Confirm build output is stable after the build**

Run:

```bash
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Expected: exit code 0 and output under `generated/assets/`.

- [ ] **Step 6: Run dashboard Rust tests**

Run:

```bash
devenv shell -- cargo test -p right-dashboard
```

Expected: all `right-dashboard` tests pass.

- [ ] **Step 7: Run final workspace test**

Run:

```bash
devenv shell -- cargo test --workspace
```

Expected: exit code 0. If unrelated in-progress workspace edits cause failures, record the failing crate/test and do not modify unrelated files.

- [ ] **Step 8: Inspect staged scope**

Run:

```bash
devenv shell -- git status --short
devenv shell -- git diff --cached --name-status
```

Expected staged paths:

```text
.gitattributes
devenv.nix
.github/workflows/build.yml
.github/workflows/tests.yml
.github/workflows/release-plz.yml
crates/right-dashboard/frontend/vite.config.ts
crates/right-dashboard/static/dashboard/index.html
crates/right-dashboard/static/dashboard/generated/assets/*
docs/architecture/modules.md
```

Expected unstaged paths may include unrelated work already present in the shared workspace. Leave them untouched.

- [ ] **Step 9: Commit**

Run:

```bash
devenv shell -- git add -- .gitattributes devenv.nix .github/workflows/build.yml .github/workflows/tests.yml .github/workflows/release-plz.yml crates/right-dashboard/frontend/vite.config.ts crates/right-dashboard/static/dashboard docs/architecture/modules.md
devenv shell -- git commit -m "chore(dashboard): mark generated static assets"
```

Expected: one commit containing only the generated-assets layout cleanup.
