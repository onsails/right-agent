{ pkgs, lib, config, inputs, ... }:

{
  packages = [
    pkgs.git
    pkgs.process-compose
    pkgs.socat
  ] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.bubblewrap
  ];

  languages.rust.enable = true;

  enterShell = ''
    echo "RightClaw dev environment"
  '';

  enterTest = ''
    cargo test --workspace
    cargo clippy --workspace -- -D warnings
  '';
}
