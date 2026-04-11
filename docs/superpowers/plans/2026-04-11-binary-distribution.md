# Binary Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Automated binary releases via release-plz (changelog + versioning) and GitHub Actions (build + publish), with install.sh updated to include OpenShell.

**Architecture:** Two GitHub Actions workflows — release-plz creates PRs with version bumps and changelogs, then creates GitHub Releases on merge. A separate build workflow triggers on release creation, compiles binaries on native runners, and uploads them as release assets. install.sh is the user-facing entry point.

**Tech Stack:** release-plz, git-cliff, GitHub Actions, cargo

---

### Task 1: Create cliff.toml

**Files:**
- Create: `cliff.toml`

- [ ] **Step 1: Create cliff.toml**

```toml
[changelog]
header = """# Changelog\n"""
body = """
{% if version -%}
    ## [{{ version | trim_start_matches(pat="v") }}] - {{ timestamp | date(format="%Y-%m-%d") }}
{% else -%}
    ## [Unreleased]
{% endif %}
{% for group, commits in commits | group_by(attribute="group") %}
    ### {{ group | upper_first }}
    {% for commit in commits %}
        - {% if commit.scope %}**{{ commit.scope }}**: {% endif %}{% if commit.breaking %}[**breaking**] {% endif %}\
            {{ commit.message | split(pat="\n") | first | upper_first | trim }}\
    {% endfor %}
{% endfor %}
"""
trim = true

[git]
conventional_commits = true
filter_unconventional = true
split_commits = false
commit_parsers = [
    { message = "^feat", group = "Features" },
    { message = "^fix", group = "Bug Fixes" },
    { message = "^doc", group = "Documentation" },
    { message = "^perf", group = "Performance" },
    { message = "^refactor", group = "Refactor" },
    { message = "^test", group = "Testing" },
    { message = "^chore\\(release\\)", skip = true },
    { message = "^chore\\(deps\\)", skip = true },
    { message = "^chore", group = "Miscellaneous" },
    { message = "^ci", group = "CI" },
]
protect_breaking_commits = false
filter_commits = false
tag_pattern = "v[0-9].*"
sort_commits = "oldest"
```

- [ ] **Step 2: Commit**

```bash
git add cliff.toml
git commit -m "ci: add git-cliff changelog config"
```

---

### Task 2: Create release-plz.toml

**Files:**
- Create: `release-plz.toml`

- [ ] **Step 1: Create release-plz.toml**

The workspace has 3 crates. Only `rightclaw-cli` produces a binary we want to release. The library crates (`rightclaw`, `rightclaw-bot`) should not be published or released independently.

```toml
[workspace]
changelog_config = "cliff.toml"
changelog_update = true
dependencies_update = false
git_release_enable = false
publish = false
semver_check = false

[[package]]
name = "rightclaw-cli"
git_release_enable = true
git_release_name = "v{{ version }}"
git_tag_name = "v{{ version }}"
changelog_path = "CHANGELOG.md"
```

Key decisions:
- `publish = false` at workspace level — no crates.io publish for any crate
- `git_release_enable = false` at workspace level, `true` only for `rightclaw-cli`
- `dependencies_update = false` — we manage deps manually
- `semver_check = false` — library crates are internal, no public API contract
- Changelog lives at repo root `CHANGELOG.md`

- [ ] **Step 2: Commit**

```bash
git add release-plz.toml
git commit -m "ci: add release-plz config for rightclaw-cli releases"
```

---

### Task 3: Create release-plz GitHub Actions workflow

**Files:**
- Create: `.github/workflows/release-plz.yml`

- [ ] **Step 1: Create .github/workflows/ directory and workflow**

```yaml
name: Release-plz

on:
  push:
    branches:
      - master

jobs:
  release-plz-release:
    name: Release-plz release
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - name: Checkout repository
        uses: actions/checkout@v6
        with:
          fetch-depth: 0
          persist-credentials: false
      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
      - name: Run release-plz
        uses: release-plz/action@v0.5
        with:
          command: release
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

  release-plz-pr:
    name: Release-plz PR
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    concurrency:
      group: release-plz-${{ github.ref }}
      cancel-in-progress: false
    steps:
      - name: Checkout repository
        uses: actions/checkout@v6
        with:
          fetch-depth: 0
          persist-credentials: false
      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
      - name: Run release-plz
        uses: release-plz/action@v0.5
        with:
          command: release-pr
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

Note: No `CARGO_REGISTRY_TOKEN` needed since we don't publish to crates.io.

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/release-plz.yml
git commit -m "ci: add release-plz workflow for automated releases"
```

---

### Task 4: Create build workflow

**Files:**
- Create: `.github/workflows/build.yml`

- [ ] **Step 1: Create the build workflow**

```yaml
name: Build binaries

on:
  release:
    types: [published]

jobs:
  build:
    name: Build ${{ matrix.target }}
    runs-on: ${{ matrix.runner }}
    strategy:
      matrix:
        include:
          - runner: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            artifact: rightclaw-x86_64-unknown-linux-gnu
          - runner: macos-14
            target: aarch64-apple-darwin
            artifact: rightclaw-aarch64-apple-darwin
    permissions:
      contents: write
    steps:
      - name: Checkout repository
        uses: actions/checkout@v6

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Build
        run: cargo build --release -p rightclaw-cli --target ${{ matrix.target }}

      - name: Rename binary
        run: cp target/${{ matrix.target }}/release/rightclaw ${{ matrix.artifact }}

      - name: Upload to release
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: gh release upload "${{ github.event.release.tag_name }}" "${{ matrix.artifact }}"
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "ci: add binary build workflow triggered on release"
```

---

### Task 5: Update install.sh

**Files:**
- Modify: `install.sh`

- [ ] **Step 1: Update Linux target from musl to gnu**

In `install_rightclaw()`, change the target mapping at line ~91:

```bash
# Before:
linux-x86_64)   target="rightclaw-x86_64-unknown-linux-musl" ;;
linux-aarch64)  target="rightclaw-aarch64-unknown-linux-musl" ;;

# After:
linux-x86_64)   target="rightclaw-x86_64-unknown-linux-gnu" ;;
linux-aarch64)  target="rightclaw-aarch64-unknown-linux-gnu" ;;
```

- [ ] **Step 2: Add OpenShell install function**

Add after `install_sandbox_deps()` function (after line ~195):

```bash
# ── Step 4: Install OpenShell ─────────────────────────────────────

install_openshell() {
  info "Installing OpenShell..."

  if command -v openshell >/dev/null 2>&1; then
    ok "OpenShell already installed: $(command -v openshell)"
    return 0
  fi

  echo "  using official installer..."
  curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh

  if command -v openshell >/dev/null 2>&1; then
    ok "OpenShell installed"
  else
    fail "OpenShell installation failed — install manually: https://github.com/NVIDIA/OpenShell"
    # Non-fatal: rightclaw works without OpenShell (sandbox: mode: none)
  fi
}
```

Note: OpenShell install failure is non-fatal with a warning. rightclaw can run agents without sandbox (`sandbox: mode: none`), so a broken OpenShell install should not block the entire setup. The user gets a clear message about what happened.

- [ ] **Step 3: Update main() to call install_openshell**

Update the `main()` function to add the OpenShell step. The order becomes:

```bash
main() {
  echo ""
  echo "${BOLD}  RightClaw Installer${RESET}"
  echo "  Multi-agent runtime for Claude Code"
  echo ""

  detect_platform
  setup_install_dir

  echo ""
  install_rightclaw
  install_process_compose
  install_openshell
  install_sandbox_deps

  echo ""
  check_bun

  echo ""
  run_init

  echo ""
  run_doctor

  echo ""
  echo "${GREEN}${BOLD}  Installation complete!${RESET}"
  echo ""
  echo "  Next steps:"
  echo "    1. Start your agents:  ${CYAN}rightclaw up${RESET}"
  echo "    2. View the TUI:       ${CYAN}rightclaw attach${RESET}"
  echo "    3. Check status:       ${CYAN}rightclaw status${RESET}"
  echo ""
  echo "  Make sure ${CYAN}$INSTALL_DIR${RESET} is in your PATH."
  echo "  Add this to your shell profile if needed:"
  echo "    ${CYAN}export PATH=\"\$HOME/.local/bin:\$PATH\"${RESET}"
  echo ""
}
```

- [ ] **Step 4: Update step comments numbering**

Update the section comments to reflect new ordering:
- Step 1: Install RightClaw (unchanged)
- Step 2: Install process-compose (unchanged)
- Step 3: Install OpenShell (new)
- Step 4: Install sandbox dependencies (was Step 3)
- Step 5: Run rightclaw init (was Step 4)
- Step 6: Run rightclaw doctor (was Step 5)

- [ ] **Step 5: Commit**

```bash
git add install.sh
git commit -m "feat: update installer with gnu targets and mandatory OpenShell install"
```

---

### Task 6: Verify and test

- [ ] **Step 1: Verify install.sh syntax**

```bash
bash -n install.sh
```

Expected: no output (valid syntax).

- [ ] **Step 2: Verify workflow YAML syntax**

```bash
# Use yq or python to validate YAML
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release-plz.yml'))"
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/build.yml'))"
```

Expected: no errors.

- [ ] **Step 3: Verify TOML syntax**

```bash
python3 -c "import tomllib; tomllib.load(open('cliff.toml', 'rb'))"
python3 -c "import tomllib; tomllib.load(open('release-plz.toml', 'rb'))"
```

Expected: no errors.

- [ ] **Step 4: Verify all files are committed**

```bash
git status
```

Expected: clean working tree (no untracked config files).
