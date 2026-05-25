# Dashboard Generated Assets Layout Design

## Context

The Telegram dashboard frontend is a Vue/Vite app under
`crates/right-dashboard/frontend/`. Its production output is checked in under
`crates/right-dashboard/static/dashboard/` and embedded into the
`right-dashboard` crate with `include_dir!`.

This means the checked-in static bundle is part of the shipped Rust binary, not
just a local development artifact. Removing it from git would require release
and local build pipelines to generate assets before compiling Rust. That is a
larger packaging change than this cleanup needs.

The problem is review clarity. Hashed files such as `index-BzimxT_t.js` look
like hand-authored source changes in GitHub diffs, even though reviewers should
treat them as generated Vite output.

## Decision

Keep committing dashboard static output, but make the generated boundary obvious.

- Add a root `.gitattributes`.
- Mark `crates/right-dashboard/static/dashboard/**` as `linguist-generated=true`.
- Keep `crates/right-dashboard/static/dashboard/index.html` at the static root.
- Move Vite chunk output from `static/dashboard/assets/` to
  `static/dashboard/generated/assets/`.
- Store generated Vite chunks under `generated/assets/` in Git LFS.
- Update the Vite config with `assetsDir: 'generated/assets'`.
- Make CI and release checkouts fetch LFS objects before Rust compilation, so
  `include_dir!` embeds real JS/CSS bytes instead of Git LFS pointer files.
- Keep existing dashboard URLs stable: `/dashboard/<agent>/` still serves the
  root `index.html`, and nested generated assets are served by the existing
  catch-all static route.

This keeps the Rust embedding model unchanged while making generated files easy
to identify in the tree and hidden by default in GitHub diffs.

## Alternatives

### Commit Static Assets In Current Location

This is the current state. It is operationally simple, but reviewers keep seeing
opaque hashed bundle files next to `index.html` with no clear generated marker.

### Stop Committing Static Assets

This is cleaner in git history, but it moves correctness into CI/release
orchestration. Every release build, local package build, and clean Rust build
would need a guaranteed frontend build step before `include_dir!` runs. That is
valid future work, but too much blast radius for a review-clarity cleanup.

### Move All Static Output Under `generated/`

This makes the tree even clearer, but moving `index.html` requires changing Rust
asset lookup or route behavior. Keeping `index.html` at the root preserves the
current route contract.

## Implementation Shape

Add `.gitattributes`:

```gitattributes
crates/right-dashboard/static/dashboard/** linguist-generated=true
crates/right-dashboard/static/dashboard/generated/assets/** filter=lfs diff=lfs merge=lfs -text linguist-generated=true
```

Update `crates/right-dashboard/frontend/vite.config.ts` so the build writes
hashed chunks into `generated/assets` while keeping `index.html` at
`static/dashboard/index.html`.

Rebuild the dashboard frontend once so committed static output matches the new
layout. Add the generated chunk files through Git LFS, and verify
`git lfs ls-files crates/right-dashboard/static/dashboard/generated/assets`
lists them.

Update build environments:

- add `git-lfs` to `devenv.nix`;
- set `lfs: true` on GitHub Actions checkouts that compile, test, package,
  release, or publish repository content containing dashboard static assets.

Update architecture docs on touch:

- `docs/architecture/modules.md` should say `static/dashboard/` is checked-in
  generated dashboard output, with Vite chunks under `generated/assets/` stored
  in Git LFS.
- `ARCHITECTURE.md` only needs a change if the route contract or crate boundary
  changes. This design does not change either.

## Verification

Targeted checks for the implementation:

- `devenv shell -- npm test --prefix crates/right-dashboard/frontend`
- `devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend`
- `devenv shell -- npm run build --prefix crates/right-dashboard/frontend`
- `devenv shell -- cargo test -p right-dashboard`

Final workspace check remains:

- `devenv shell -- cargo test --workspace`

If the workspace is mixed with unrelated in-progress edits, record unrelated
workspace failures instead of fixing or reverting other people's files.

## Out Of Scope

- Removing committed dashboard static assets.
- Building frontend assets from `build.rs`.
- Changing release packaging.
- Changing dashboard route URLs.
- Adding a separate dashboard process.
- Storing `static/dashboard/index.html` in Git LFS.
