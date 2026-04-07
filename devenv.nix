{ pkgs, lib, config, inputs, ... }:

{
  packages = [
    pkgs.git
    pkgs.process-compose
    pkgs.socat
    pkgs.ripgrep          # SBOX-04: CC sandbox rg check; must be in agent launch PATH
    pkgs.grpcurl
    pkgs.protobuf
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
