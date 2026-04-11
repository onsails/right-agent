# Binary Distribution Design

## Problem

RightClaw has no automated release pipeline. `install.sh` exists and expects prebuilt binaries at GitHub Releases, but no CI builds or publishes them. Homebrew requires significant traction (stars). We need a one-liner install that works today.

## Decision

**release-plz** for automated versioning + changelog, separate GitHub Actions workflow for binary builds.

## Release Flow

```
conventional commit push to master
  → release-plz creates PR: version bump in Cargo.toml + CHANGELOG.md
  → review changelog, merge PR
  → release-plz creates GitHub Release with tag v0.x.x
  → build workflow triggers on: release [published]
  → builds 2 binaries, uploads to release assets
```

## Components

### 1. release-plz.toml

- Only release `rightclaw-cli` package (the binary crate)
- No crates.io publish
- Reference `cliff.toml` for changelog format
- `git_release_enable = true`

### 2. cliff.toml

Conventional commits grouped by type (feat, fix, refactor, etc.). Standard git-cliff config.

### 3. .github/workflows/release-plz.yml

Triggers on push to master. Two jobs:

- **release-plz**: runs `release-plz release-pr` and `release-plz release`
- Requires `GITHUB_TOKEN` with contents write + PR permissions

### 4. .github/workflows/build.yml

Triggers on `release: [published]`.

Build matrix:

| Runner | Target | Artifact name |
|--------|--------|---------------|
| `ubuntu-latest` | `x86_64-unknown-linux-gnu` | `rightclaw-x86_64-unknown-linux-gnu` |
| `macos-14` | `aarch64-apple-darwin` | `rightclaw-aarch64-apple-darwin` |

Steps per job:
1. Checkout at release tag
2. Install Rust toolchain
3. `cargo build --release -p rightclaw-cli`
4. Rename `target/release/rightclaw` to target-specific name
5. `gh release upload <tag> <artifact>`

### 5. install.sh updates

Current state: expects musl targets, no OpenShell install.

Changes:
- **Targets**: `linux-x86_64` maps to `rightclaw-x86_64-unknown-linux-gnu` (was musl)
- **OpenShell install**: mandatory step, runs their official installer:
  ```bash
  curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh
  ```
- **Order**: rightclaw → process-compose → OpenShell → sandbox deps → init → doctor

### 6. One-liner

```bash
curl -LsSf https://raw.githubusercontent.com/onsails/rightclaw/master/install.sh | sh
```

Unchanged — already works once binaries exist on releases.

## What We Don't Do

- crates.io publish (not yet)
- Homebrew tap (needs stars)
- Nix flake
- cargo-dist (unnecessary abstraction over simple workflow)
- linux-aarch64, darwin-x86_64 targets (add on demand)
- Custom installer generation (install.sh covers our needs)

## Files to Create/Modify

| File | Action |
|------|--------|
| `release-plz.toml` | Create |
| `cliff.toml` | Create |
| `.github/workflows/release-plz.yml` | Create |
| `.github/workflows/build.yml` | Create |
| `install.sh` | Modify (gnu targets, add OpenShell step) |
