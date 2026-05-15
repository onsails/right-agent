{ pkgs, lib, config, inputs, ... }:

{
  packages = with pkgs; [
    process-compose
    socat
    ripgrep          # SBOX-04: CC sandbox rg check; must be in agent launch PATH
    grpcurl
    protobuf
    cmake            # required by whisper-rs-sys build script
    sccache
    actionlint
  ] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.bubblewrap
  ];

  languages.rust.enable = true;

  git-hooks.hooks.rustfmt.enable = true;

  env.RUSTC_WRAPPER = "sccache";
  # Keep devenv builds separate from system-profile Cargo calls, which
  # otherwise frequently invalidate the shared target cache.
  env.CARGO_TARGET_DIR = "target/devenv";

  enterShell = ''
    echo "Right Agent dev environment"
  '';

  enterTest = ''
    cargo test --workspace
    cargo clippy --workspace -- -D warnings
  '';
}
