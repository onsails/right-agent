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
    pnpm
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
    current_root="''${DEVENV_ROOT:-$PWD}"
    if git_common_dir="$(${pkgs.git}/bin/git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)"; then
      repo_root="$(dirname "$git_common_dir")"
      if [ "$repo_root" = "$current_root" ]; then
        export SCCACHE_BASEDIRS="$current_root"
      else
        export SCCACHE_BASEDIRS="$current_root:$repo_root"
      fi
    else
      export SCCACHE_BASEDIRS="$current_root"
    fi

    case "$-" in
      *i*) echo "Right Agent dev environment" ;;
    esac
  '';

  enterTest = ''
    cargo test --workspace
    cargo clippy --workspace -- -D warnings
  '';
}
