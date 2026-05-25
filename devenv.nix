{ pkgs, lib, config, inputs, ... }:

{
  packages = with pkgs; [
    process-compose
    socat
    ripgrep          # SBOX-04: CC sandbox rg check; must be in agent launch PATH
    grpcurl
    protobuf
    ffmpeg
    pkg-config
    openssl
    cmake            # required by whisper-rs-sys build script
    sccache
    actionlint
    nodejs
    git-lfs
  ] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.bubblewrap
  ];

  languages.rust.enable = true;

  git-hooks.hooks.rustfmt.enable = true;

  env.RUSTC_WRAPPER = "sccache";
  env.LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  # Keep devenv builds separate from system-profile Cargo calls, which
  # otherwise frequently invalidate the shared target cache.
  env.CARGO_TARGET_DIR = "target/devenv";

  enterShell = ''
    case "$-" in
      *i*) echo "Right Agent dev environment" ;;
    esac
  '';

  enterTest = ''
    cargo test --workspace
    cargo clippy --workspace -- -D warnings
  '';
}
