{ pkgs, lib, config, inputs, ... }:

{
  packages = [
    pkgs.git
    pkgs.process-compose
  ];

  languages.rust = {
    enable = true;
    channel = "stable";
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" ];
  };

  enterShell = ''
    echo "RightClaw dev environment"
    rustc --version
    cargo --version
    # Ensure 'claude' is in PATH for rightclaw doctor/up
    if ! command -v claude &>/dev/null; then
      if command -v claude-bun &>/dev/null; then
        mkdir -p "$DEVENV_STATE/bin"
        ln -sf "$(command -v claude-bun)" "$DEVENV_STATE/bin/claude"
        export PATH="$DEVENV_STATE/bin:$PATH"
      fi
    fi
  '';

  enterTest = ''
    cargo test --workspace
    cargo clippy --workspace -- -D warnings
  '';
}
