#!/usr/bin/env bash
set -euo pipefail

# ── Right Agent Installer ──────────────────────────────────────────
#
# Installs:
#   1. right            - Multi-agent runtime CLI
#   2. process-compose  - Process orchestrator with TUI
#   3. NVIDIA OpenShell - Sandbox runtime for AI agents
#
# Prerequisites (install separately before running this script):
#   - Claude Code CLI     https://docs.anthropic.com/en/docs/claude-code
#   - cloudflared         https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/
#
# Usage:
#   curl -LsSf https://raw.githubusercontent.com/onsails/right-agent/master/install.sh | sh
#
# Environment variables:
#   RIGHT_VERSION      - Version to install (default: latest)
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

# ── Step 1: Install Right Agent ────────────────────────────────────

install_right() {
  if command -v right >/dev/null 2>&1; then
    info "Updating right..."
  else
    info "Installing right..."
  fi

  local version="${RIGHT_VERSION:-latest}"
  local target
  local download_url
  local http_code=""

  case "${PLATFORM}-${ARCH}" in
    linux-x86_64)   target="right-x86_64-unknown-linux-gnu" ;;
    linux-aarch64)  target="right-aarch64-unknown-linux-gnu" ;;
    darwin-aarch64) target="right-aarch64-apple-darwin" ;;
    *)              die "Unsupported platform: ${PLATFORM}-${ARCH}. Right Agent ships for linux-x86_64, linux-aarch64, and darwin-aarch64 (Apple Silicon)." ;;
  esac

  if [ "$version" = "latest" ]; then
    download_url="https://github.com/onsails/right-agent/releases/latest/download/${target}"
  else
    download_url="https://github.com/onsails/right-agent/releases/download/${version}/${target}"
  fi

  echo "  downloading: $download_url"

  http_code=$(curl -LsSf -w '%{http_code}' -o "$INSTALL_DIR/right" "$download_url" 2>/dev/null) || http_code="000"

  if [ "$http_code" = "200" ]; then
    chmod +x "$INSTALL_DIR/right"
    ok "right installed to $INSTALL_DIR/right"
    return 0
  fi

  # Fallback: build from source with cargo
  warn "GitHub release download failed (HTTP $http_code), trying cargo install..."

  if ! command -v cargo >/dev/null 2>&1; then
    die "Cannot install right: no prebuilt binary available and cargo is not installed.
    Install Rust first: https://rustup.rs"
  fi

  # If we're inside a cloned repo, build from path
  if [ -f "crates/right-agent/Cargo.toml" ]; then
    echo "  building from local source..."
    cargo install --path crates/right-agent --root "$HOME/.local" --force
  else
    echo "  installing from crates.io..."
    cargo install right-agent --root "$HOME/.local" --force
  fi

  if [ -f "$INSTALL_DIR/right" ]; then
    ok "right built and installed to $INSTALL_DIR/right"
  else
    die "Failed to install right via cargo"
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
  if command -v openshell >/dev/null 2>&1; then
    info "Updating OpenShell..."
  else
    info "Installing OpenShell..."
  fi

  echo "  using official installer..."
  curl -LsSf https://raw.githubusercontent.com/NVIDIA/OpenShell/main/install.sh | sh

  if command -v openshell >/dev/null 2>&1; then
    ok "OpenShell installed"
  else
    fail "OpenShell installation failed — install manually: https://github.com/NVIDIA/OpenShell"
  fi
}

# ── Step 4: Run right init ─────────────────────────────────────────

run_init() {
  if [ -d "$HOME/.right" ]; then
    ok "~/.right already exists, skipping init"
    return 0
  fi

  info "Running right init..."

  # Use full path to avoid PATH resolution issues (Pitfall 6)
  "$INSTALL_DIR/right" init
}

# ── Step 5: Run right doctor ───────────────────────────────────────

run_doctor() {
  info "Running right doctor..."

  # Use full path to avoid PATH resolution issues (Pitfall 6)
  "$INSTALL_DIR/right" doctor
}

# ── Main ───────────────────────────────────────────────────────────

main() {
  echo ""
  echo "${BOLD}  Right Agent Installer${RESET}"
  echo "  Multi-agent runtime for Claude Code"
  echo ""

  detect_platform
  setup_install_dir

  echo ""
  install_right
  install_process_compose
  install_openshell

  echo ""
  run_init

  echo ""
  run_doctor

  echo ""
  echo "${GREEN}${BOLD}  Installation complete!${RESET}"
  echo ""
  echo "  Next steps:"
  echo "    1. Start your agents:  ${CYAN}right up${RESET}"
  echo "    2. View the TUI:       ${CYAN}right attach${RESET}"
  echo "    3. Check status:       ${CYAN}right status${RESET}"
  echo ""
  echo "  Make sure ${CYAN}$INSTALL_DIR${RESET} is in your PATH."
  echo "  Add this to your shell profile if needed:"
  echo "    ${CYAN}export PATH=\"\$HOME/.local/bin:\$PATH\"${RESET}"
  echo ""
}

main "$@"
