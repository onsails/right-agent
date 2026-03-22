#!/usr/bin/env bash
set -euo pipefail

# ── RightClaw Installer ────────────────────────────────────────────
#
# Installs RightClaw and its dependencies:
#   1. rightclaw    - Multi-agent runtime CLI
#   2. process-compose - Process orchestrator with TUI
#   3. OpenShell    - NVIDIA sandbox runtime
#
# Usage:
#   curl -LsSf https://raw.githubusercontent.com/onsails/rightclaw/main/install.sh | sh
#
# Environment variables:
#   RIGHTCLAW_VERSION  - Version to install (default: latest)
#   INSTALL_DIR        - Binary install directory (default: ~/.local/bin)

# ── Colors ─────────────────────────────────────────────────────────

if [ -t 1 ] && command -v tput >/dev/null 2>&1; then
  BOLD="$(tput bold)"
  GREEN="$(tput setaf 2)"
  YELLOW="$(tput setaf 3)"
  RED="$(tput setaf 1)"
  CYAN="$(tput setaf 6)"
  RESET="$(tput sgr0)"
else
  BOLD="" GREEN="" YELLOW="" RED="" CYAN="" RESET=""
fi

info()  { echo "${BOLD}${CYAN}==> ${RESET}${BOLD}$*${RESET}"; }
ok()    { echo "  ${GREEN}ok${RESET}  $*"; }
warn()  { echo "  ${YELLOW}warn${RESET}  $*"; }
fail()  { echo "  ${RED}FAIL${RESET}  $*"; }
die()   { echo "${RED}error:${RESET} $*" >&2; exit 1; }

# ── Platform Detection ─────────────────────────────────────────────

detect_platform() {
  local os arch

  os="$(uname -s)"
  case "$os" in
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="darwin" ;;
    *)      die "Unsupported OS: $os (only Linux and macOS are supported)" ;;
  esac

  arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64)  ARCH="x86_64" ;;
    arm64|aarch64) ARCH="aarch64" ;;
    *)             die "Unsupported architecture: $arch (only x86_64 and arm64 are supported)" ;;
  esac

  echo "  platform: ${PLATFORM}-${ARCH}"
}

# ── Install Directory ──────────────────────────────────────────────

INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

setup_install_dir() {
  if [ ! -d "$INSTALL_DIR" ]; then
    info "Creating install directory: $INSTALL_DIR"
    mkdir -p "$INSTALL_DIR"
  fi

  # Ensure install dir is in PATH for this session
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) export PATH="$INSTALL_DIR:$PATH" ;;
  esac
}

# ── Step 1: Install RightClaw ──────────────────────────────────────

install_rightclaw() {
  info "Installing rightclaw..."

  if command -v rightclaw >/dev/null 2>&1; then
    ok "rightclaw already installed: $(command -v rightclaw)"
    return 0
  fi

  local version="${RIGHTCLAW_VERSION:-latest}"
  local target
  local download_url

  case "${PLATFORM}-${ARCH}" in
    linux-x86_64)   target="rightclaw-x86_64-unknown-linux-musl" ;;
    linux-aarch64)  target="rightclaw-aarch64-unknown-linux-musl" ;;
    darwin-x86_64)  target="rightclaw-x86_64-apple-darwin" ;;
    darwin-aarch64) target="rightclaw-aarch64-apple-darwin" ;;
    *)              die "No prebuilt binary for ${PLATFORM}-${ARCH}" ;;
  esac

  if [ "$version" = "latest" ]; then
    download_url="https://github.com/onsails/rightclaw/releases/latest/download/${target}"
  else
    download_url="https://github.com/onsails/rightclaw/releases/download/${version}/${target}"
  fi

  echo "  downloading: $download_url"

  local http_code
  http_code=$(curl -LsSf -w '%{http_code}' -o "$INSTALL_DIR/rightclaw" "$download_url" 2>/dev/null) || http_code="000"

  if [ "$http_code" = "200" ]; then
    chmod +x "$INSTALL_DIR/rightclaw"
    ok "rightclaw installed to $INSTALL_DIR/rightclaw"
    return 0
  fi

  # Fallback: build from source with cargo
  warn "GitHub release download failed (HTTP $http_code), trying cargo install..."

  if ! command -v cargo >/dev/null 2>&1; then
    die "Cannot install rightclaw: no prebuilt binary available and cargo is not installed.
    Install Rust first: https://rustup.rs"
  fi

  # If we're inside a cloned repo, build from path
  if [ -f "crates/rightclaw-cli/Cargo.toml" ]; then
    echo "  building from local source..."
    cargo install --path crates/rightclaw-cli --root "$HOME/.local" --force
  else
    echo "  installing from crates.io..."
    cargo install rightclaw-cli --root "$HOME/.local" --force
  fi

  if [ -f "$INSTALL_DIR/rightclaw" ]; then
    ok "rightclaw built and installed to $INSTALL_DIR/rightclaw"
  else
    die "Failed to install rightclaw via cargo"
  fi
}

# ── Step 2: Install process-compose ────────────────────────────────

install_process_compose() {
  info "Installing process-compose..."

  if command -v process-compose >/dev/null 2>&1; then
    ok "process-compose already installed: $(command -v process-compose)"
    return 0
  fi

  echo "  using official installer..."
  curl -LsSf https://raw.githubusercontent.com/F1bonacc1/process-compose/main/scripts/get-pc.sh \
    | sh -s -- -b "$INSTALL_DIR"

  if [ -f "$INSTALL_DIR/process-compose" ]; then
    ok "process-compose installed to $INSTALL_DIR/process-compose"
  else
    die "Failed to install process-compose"
  fi
}

# ── Step 3: Install OpenShell ──────────────────────────────────────

install_openshell() {
  info "Installing OpenShell..."

  if command -v openshell >/dev/null 2>&1; then
    ok "openshell already installed: $(command -v openshell)"
    return 0
  fi

  echo "  using official installer (v0.0.13)..."
  curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh \
    | OPENSHELL_VERSION=v0.0.13 sh

  if command -v openshell >/dev/null 2>&1; then
    ok "openshell installed"
  else
    warn "openshell install completed but binary not found in PATH"
    echo "       You may need to add its install location to your PATH"
  fi
}

# ── Step 4: Run rightclaw init ─────────────────────────────────────

run_init() {
  info "Running rightclaw init..."

  # Use full path to avoid PATH resolution issues (Pitfall 6)
  "$INSTALL_DIR/rightclaw" init
}

# ── Step 5: Run rightclaw doctor ───────────────────────────────────

run_doctor() {
  info "Running rightclaw doctor..."

  # Use full path to avoid PATH resolution issues (Pitfall 6)
  "$INSTALL_DIR/rightclaw" doctor
}

# ── Bun Check ──────────────────────────────────────────────────────

check_bun() {
  if ! command -v bun >/dev/null 2>&1; then
    echo ""
    warn "bun is not installed"
    echo "       Bun is required for the Telegram channel plugin."
    echo "       Install it: ${CYAN}curl -fsSL https://bun.sh/install | bash${RESET}"
    echo ""
  fi
}

# ── Main ───────────────────────────────────────────────────────────

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

main "$@"
