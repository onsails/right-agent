{ ... }:
{
  # Site toolchain lives in an opt-in profile so the base Rust shell stays lean.
  # Activate with: devenv --profile site shell
  profiles.site.module = { pkgs, ... }: {
    packages = [
      pkgs.bun       # primary package manager and script runner
      pkgs.nodejs    # fallback runtime for `astro build` if sharp trips under Bun
    ];

    scripts.site-dev.exec = ''cd "$DEVENV_ROOT/site" && bun run dev'';
    scripts.site-build.exec = ''cd "$DEVENV_ROOT/site" && bun run build'';
    scripts.site-check.exec = ''cd "$DEVENV_ROOT/site" && bun run check'';
  };
}
