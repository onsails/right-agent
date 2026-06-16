# PostHog Site Analytics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add PostHog pageview and autocapture analytics to both the Astro landing page and Starlight docs portal without enabling session replay or hard-coding project keys.

**Architecture:** A shared `PostHog.astro` component renders the official inline browser snippet only in production builds when public build-time env vars are present. The landing layout imports that component directly; Starlight gets it through a `Head` component override that preserves Starlight's default head content. A small Node assertion script checks built HTML for enabled and disabled builds, including both `/` and `/docs/`.

**Tech Stack:** Astro 6, Starlight, Bun, GitHub Pages Actions variables, PostHog browser snippet.

---

## File Map

- Create `site/src/components/PostHog.astro`: shared PostHog snippet, gated by production build mode, `PUBLIC_POSTHOG_KEY`, and `PUBLIC_POSTHOG_HOST`.
- Create `site/src/components/StarlightHead.astro`: wraps Starlight's default `Head` and appends `PostHog`.
- Create `site/scripts/assert-posthog-build.mjs`: build-output assertion for enabled and disabled analytics.
- Modify `site/src/layouts/Landing.astro`: import and render `PostHog` in `<head>`.
- Modify `site/astro.config.mjs`: register the Starlight `Head` override without removing the existing `Banner` override.
- Modify `site/package.json`: add a `test:analytics` script.
- Modify `.github/workflows/static.yml`: pass repository Actions variables into the site build step.

## Task 1: Add Build-Output Assertions

**Files:**
- Create: `site/scripts/assert-posthog-build.mjs`
- Modify: `site/package.json`

- [ ] **Step 1: Create the failing assertion script**

Create `site/scripts/assert-posthog-build.mjs`:

```javascript
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const scriptDir = dirname(fileURLToPath(import.meta.url));
const siteRoot = dirname(scriptDir);
const distRoot = join(siteRoot, 'dist');

const expectEnabled = process.argv.includes('--expect-enabled');
const expectDisabled = process.argv.includes('--expect-disabled');

if (expectEnabled === expectDisabled) {
  console.error('Pass exactly one mode: --expect-enabled or --expect-disabled');
  process.exit(2);
}

const expectedKey = process.env.PUBLIC_POSTHOG_KEY || 'phc_posthog_test_key';
const expectedHost = process.env.PUBLIC_POSTHOG_HOST || 'https://us.i.posthog.com';

const pages = [
  ['landing', join(distRoot, 'index.html')],
  ['docs', join(distRoot, 'docs', 'index.html')],
];

function assert(condition, message) {
  if (!condition) {
    console.error(message);
    process.exitCode = 1;
  }
}

for (const [name, path] of pages) {
  assert(existsSync(path), `${name}: missing built file at ${path}`);
  if (!existsSync(path)) continue;

  const html = readFileSync(path, 'utf8');

  if (expectDisabled) {
    assert(!html.includes('window.posthog'), `${name}: PostHog snippet should be absent when env is missing`);
    assert(!html.includes('posthog.init'), `${name}: PostHog init should be absent when env is missing`);
    continue;
  }

  const required = [
    'window.posthog',
    'posthog.init',
    `data-posthog-key="${expectedKey}"`,
    `data-posthog-host="${expectedHost}"`,
    "defaults: '2026-01-30'",
    "capture_pageview: 'history_change'",
    'autocapture: true',
    'disable_session_recording: true',
  ];

  for (const needle of required) {
    assert(html.includes(needle), `${name}: missing ${needle}`);
  }
}

if (process.exitCode) {
  process.exit(process.exitCode);
}
```

- [ ] **Step 2: Add the package script**

In `site/package.json`, change the `scripts` object to:

```json
{
  "dev": "astro dev",
  "build": "astro build",
  "preview": "astro preview",
  "check": "astro check",
  "test:analytics": "node ./scripts/assert-posthog-build.mjs"
}
```

- [ ] **Step 3: Run a disabled build baseline**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && bun install --frozen-lockfile && rm -rf dist && env -u PUBLIC_POSTHOG_KEY -u PUBLIC_POSTHOG_HOST bun run build'
```

Expected: PASS. This records the current site still builds before analytics code exists.

- [ ] **Step 4: Verify the disabled assertion currently passes**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && bun run test:analytics -- --expect-disabled'
```

Expected: PASS because no PostHog code has been added yet.

- [ ] **Step 5: Verify the enabled assertion currently fails**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && PUBLIC_POSTHOG_KEY=phc_posthog_test_key PUBLIC_POSTHOG_HOST=https://us.i.posthog.com bun run test:analytics -- --expect-enabled'
```

Expected: FAIL with missing `window.posthog` or `posthog.init`, proving the assertion detects the behavior change.

- [ ] **Step 6: Commit the assertion harness**

Run:

```bash
git add site/scripts/assert-posthog-build.mjs site/package.json
git commit -m "test(site): assert posthog build output"
```

## Task 2: Add the Shared PostHog Component

**Files:**
- Create: `site/src/components/PostHog.astro`

- [ ] **Step 1: Create the gated inline snippet**

Create `site/src/components/PostHog.astro`:

```astro
---
const posthogKey = import.meta.env.PUBLIC_POSTHOG_KEY;
const posthogHost = import.meta.env.PUBLIC_POSTHOG_HOST;
const enabled = import.meta.env.PROD && Boolean(posthogKey && posthogHost);
---
{enabled && (
  <script
    is:inline
    data-posthog-key={posthogKey}
    data-posthog-host={posthogHost}
  >
    !function(t,e){var o,n,p,r;e.__SV||(window.posthog=e,e._i=[],e.init=function(i,s,a){function g(t,e){var o=e.split(".");2==o.length&&(t=t[o[0]],e=o[1]),t[e]=function(){t.push([e].concat(Array.prototype.slice.call(arguments,0)))}}(p=t.createElement("script")).type="text/javascript",p.async=!0,p.src=s.api_host.replace(".i.posthog.com","-assets.i.posthog.com")+"/static/array.js",(r=t.getElementsByTagName("script")[0]).parentNode.insertBefore(p,r);var u=e;for(void 0!==a?u=e[a]=[]:a="posthog",u.people=u.people||[],u.toString=function(t){var e="posthog";return"posthog"!==a&&(e+="."+a),t||(e+=" (stub)"),e},u.people.toString=function(){return u.toString(1)+".people (stub)"},o="init capture register register_once register_for_session unregister opt_out_capturing has_opted_out_capturing opt_in_capturing reset isFeatureEnabled getFeatureFlag getFeatureFlagPayload reloadFeatureFlags group identify setPersonProperties setPersonPropertiesForFlags resetPersonPropertiesForFlags setGroupPropertiesForFlags resetGroupPropertiesForFlags resetGroups onFeatureFlags addFeatureFlagsHandler onSessionId getSurveys getActiveMatchingSurveys renderSurvey canRenderSurvey getNextSurveyStep".split(" "),n=0;n<o.length;n++)g(u,o[n]);e._i.push([i,s,a])},e.__SV=1)}(document,window.posthog||[]);

    var currentScript = document.currentScript;
    var posthogKey = currentScript && currentScript.dataset.posthogKey;
    var posthogHost = currentScript && currentScript.dataset.posthogHost;

    if (posthogKey && posthogHost) {
      posthog.init(posthogKey, {
        api_host: posthogHost,
        defaults: '2026-01-30',
        capture_pageview: 'history_change',
        autocapture: true,
        disable_session_recording: true,
      });
    }
  </script>
)}
```

- [ ] **Step 2: Run the disabled build check**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && rm -rf dist && env -u PUBLIC_POSTHOG_KEY -u PUBLIC_POSTHOG_HOST bun run build && bun run test:analytics -- --expect-disabled'
```

Expected: PASS. The component exists but emits no snippet without env vars.

- [ ] **Step 3: Commit the component**

Run:

```bash
git add site/src/components/PostHog.astro
git commit -m "feat(site): add gated posthog component"
```

## Task 3: Wire Analytics Into Landing and Docs

**Files:**
- Create: `site/src/components/StarlightHead.astro`
- Modify: `site/src/layouts/Landing.astro`
- Modify: `site/astro.config.mjs`

- [ ] **Step 1: Add the Starlight head wrapper**

Create `site/src/components/StarlightHead.astro`:

```astro
---
import Default from '@astrojs/starlight/components/Head.astro';
import PostHog from './PostHog.astro';
---
<Default><slot /></Default>
<PostHog />
```

- [ ] **Step 2: Add PostHog to the landing layout**

In `site/src/layouts/Landing.astro`, update the frontmatter imports:

```astro
---
import '../styles/landing.css';
import PostHog from '../components/PostHog.astro';
import { getStars, formatStars } from '../lib/github';
interface Props { title?: string; description?: string; }
const {
  title = 'right agent',
  description = 'an ai agent you run by messaging it. credentials stay outside the box.',
} = Astro.props;
const starLabel = formatStars(await getStars());
---
```

Then add `<PostHog />` inside the existing `<head>` after the favicon:

```astro
    <link rel="icon" href="/favicon.svg" />
    <PostHog />
```

- [ ] **Step 3: Register the Starlight Head override**

In `site/astro.config.mjs`, update the existing `components` block:

```javascript
      components: {
        Banner: './src/components/DocsBanner.astro',
        Head: './src/components/StarlightHead.astro',
      },
```

- [ ] **Step 4: Run the enabled analytics assertion**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && rm -rf dist && PUBLIC_POSTHOG_KEY=phc_posthog_test_key PUBLIC_POSTHOG_HOST=https://us.i.posthog.com bun run build && PUBLIC_POSTHOG_KEY=phc_posthog_test_key PUBLIC_POSTHOG_HOST=https://us.i.posthog.com bun run test:analytics -- --expect-enabled'
```

Expected: PASS. The built landing page and docs index both contain the gated PostHog snippet, config data attributes, pageview mode, autocapture, and disabled session recording.

- [ ] **Step 5: Re-run the disabled analytics assertion**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && rm -rf dist && env -u PUBLIC_POSTHOG_KEY -u PUBLIC_POSTHOG_HOST bun run build && bun run test:analytics -- --expect-disabled'
```

Expected: PASS. The snippet remains absent when env vars are missing.

- [ ] **Step 6: Commit landing and docs wiring**

Run:

```bash
git add site/src/components/StarlightHead.astro site/src/layouts/Landing.astro site/astro.config.mjs
git commit -m "feat(site): wire posthog into landing and docs"
```

## Task 4: Pass GitHub Actions Variables Into the Site Build

**Files:**
- Modify: `.github/workflows/static.yml`

- [ ] **Step 1: Add build-step environment variables**

In `.github/workflows/static.yml`, change the `Build site (Bun, in the devenv site profile)` step to:

```yaml
      - name: Build site (Bun, in the devenv site profile)
        env:
          PUBLIC_POSTHOG_KEY: ${{ vars.PUBLIC_POSTHOG_KEY }}
          PUBLIC_POSTHOG_HOST: ${{ vars.PUBLIC_POSTHOG_HOST }}
        run: |
          devenv --profile site shell -- bash -lc \
            'cd site && bun install --frozen-lockfile && bun run build'
```

- [ ] **Step 2: Commit workflow env pass-through**

Run:

```bash
git add .github/workflows/static.yml
git commit -m "ci(site): pass posthog vars to pages build"
```

## Task 5: Final Verification

**Files:**
- Verify only.

- [ ] **Step 1: Run Astro type/content checks**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && bun run check'
```

Expected: PASS.

- [ ] **Step 2: Run enabled site build assertion**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && rm -rf dist && PUBLIC_POSTHOG_KEY=phc_posthog_test_key PUBLIC_POSTHOG_HOST=https://us.i.posthog.com bun run build && PUBLIC_POSTHOG_KEY=phc_posthog_test_key PUBLIC_POSTHOG_HOST=https://us.i.posthog.com bun run test:analytics -- --expect-enabled'
```

Expected: PASS.

- [ ] **Step 3: Run disabled site build assertion**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && rm -rf dist && env -u PUBLIC_POSTHOG_KEY -u PUBLIC_POSTHOG_HOST bun run build && bun run test:analytics -- --expect-disabled'
```

Expected: PASS.

- [ ] **Step 4: Run final workspace tests**

Run:

```bash
devenv shell -- cargo nextest run --workspace
devenv shell -- cargo test --doc --workspace
```

Expected: PASS. Record any pre-existing failures if they appear, but do not skip this step.

- [ ] **Step 5: Inspect final diff**

Run:

```bash
git status --short
git diff --stat HEAD~4..HEAD
```

Expected: only the analytics files, workflow change, and the plan/spec commits are related to this work. Existing unrelated untracked docs remain untouched.
