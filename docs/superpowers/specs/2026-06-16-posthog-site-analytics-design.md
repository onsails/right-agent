# PostHog Site Analytics: Design Spec

**Date:** 2026-06-16
**Status:** Approved for planning
**Topic:** PostHog analytics for the public website and docs portal

## Goal

Add PostHog web analytics to the Astro public site so the landing page and
Starlight docs report pageviews and normal interaction events. Keep the first
pass privacy-conscious: enable autocapture, disable session replay, and avoid
committing project-specific keys.

## Decisions

- Use PostHog's browser snippet pattern for Astro, adapted for this repo.
- Store configuration in public build-time environment variables:
  `PUBLIC_POSTHOG_KEY` and `PUBLIC_POSTHOG_HOST`.
- Track both the custom landing page at `/` and the Starlight docs under
  `/docs/*`.
- Enable pageview tracking and autocapture.
- Disable session recording/replay in code.
- Do not add custom product events in the first pass.

## Architecture

The site remains one Astro project in `site/`.

A small analytics component under `site/src/components/` owns the PostHog
snippet. It reads public Astro environment values and emits no script when the
key is missing. This keeps local development and forks from sending accidental
events.

The landing layout includes the analytics component in its document head. The
docs portal gets the same component through Starlight's head/component override
surface, because wrapping `index.astro` alone would not cover Starlight pages.

## Configuration

Local testing uses `site/.env.local`, which is not committed:

```dotenv
PUBLIC_POSTHOG_KEY=phc_...
PUBLIC_POSTHOG_HOST=https://us.i.posthog.com
```

Production GitHub Pages uses repository-level Actions variables with the same
names. The Pages workflow exposes them to the Astro build environment.

The PostHog project key is public browser configuration, not a secret. It still
must not be hard-coded into committed source because each deploy target should
control its own project.

## Tracking Scope

PostHog initialization uses:

- `api_host` from `PUBLIC_POSTHOG_HOST`.
- `defaults: '2026-01-30'`, matching the current PostHog Astro snippet.
- `capture_pageview: 'history_change'` so future client-side navigation changes
  keep reporting pageviews correctly.
- `autocapture: true`.
- `disable_session_recording: true`.

If a later page adds sensitive controls or free-form inputs, mark those elements
or containers with PostHog's no-capture class instead of turning off analytics
globally.

## Error Handling

Analytics is best-effort. Missing environment variables disable the snippet.
Runtime analytics failures must not affect rendering, navigation, docs search,
or install-copy behavior.

## Verification

Targeted site checks after implementation:

- `devenv shell -- bun run --cwd site check`
- `devenv shell -- bun run --cwd site build`
- Inspect built output or preview pages to confirm the PostHog script appears on
  `/` and `/docs/`.
- Run a local preview with `PUBLIC_POSTHOG_KEY` and `PUBLIC_POSTHOG_HOST` set,
  then confirm pageview/autocapture requests fire on landing and docs pages.

Final workspace verification after code work still follows the repo rule:

- `devenv shell -- cargo nextest run --workspace`
- `devenv shell -- cargo test --doc --workspace`

## Non-goals

- No session replay.
- No custom event taxonomy.
- No analytics for the Rust dashboard or Telegram Mini App.
- No consent banner in this pass; add it only if deployment/legal requirements
  demand it.
