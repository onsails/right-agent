{ pkgs, lib, config, inputs, ... }:

{
  packages = with pkgs; [
    process-compose
    socat
    ripgrep          # SBOX-04: CC sandbox rg check; must be in agent launch PATH
    grpcurl
    protobuf
    # CLI-only ffmpeg for STT audio decode. right-stt only shells out to the
    # `ffmpeg` binary to decode audio (crates/right-stt/src/lib.rs), so the
    # GUI/X11/SDL/pango deps in the full `ffmpeg` are dead weight. Headless
    # drops the closure from ~1.0 GiB / 307 paths to ~299 MiB / 115 paths,
    # ~700 MiB less to download on every fresh CI runner.
    ffmpeg-headless
    pkg-config
    openssl
    cmake            # required by whisper-rs-sys build script
    sccache
    actionlint
    nodejs
    pnpm
    git-lfs
    curl             # libcurl required to link release-plz (installed in enterShell)
    cargo-nextest    # recommended test runner (process-per-test, faster CI/local loop)
  ] ++ lib.optionals pkgs.stdenv.isLinux [
    pkgs.bubblewrap
  ];

  languages.rust = {
    enable = true;
    # Track the latest stable Rust via the rust-overlay channel rather than
    # nixpkgs' pinned rustc, which can lag behind language features the
    # workspace and its dependencies rely on.
    channel = "stable";
  };

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

    # release-plz is marked broken in current nixpkgs on Darwin (libcurl
    # link failure). Install it once into a devenv-local cargo root so it
    # is on PATH for `release-plz set-version`, changelog/version work.
    release_plz_root="$current_root/.devenv/cargo-tools"
    export PATH="$release_plz_root/bin:$PATH"
    if ! command -v release-plz >/dev/null 2>&1; then
      echo "Installing release-plz into $release_plz_root (one-time)..."
      cargo install --locked --root "$release_plz_root" release-plz
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
