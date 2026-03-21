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
  '';

  enterTest = ''
    cargo test --workspace
    cargo clippy --workspace -- -D warnings
  '';
}
