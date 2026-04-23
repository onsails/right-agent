{ pkgs, lib, config, inputs, ... }:

{
  packages = with pkgs; [
    process-compose
    socat
    ripgrep          # SBOX-04: CC sandbox rg check; must be in agent launch PATH
    grpcurl
    protobuf
    cmake            # required by whisper-rs-sys build script
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
