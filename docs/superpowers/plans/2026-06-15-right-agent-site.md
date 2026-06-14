# Right Agent Site Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the public Right Agent website (brand landing at `/` plus Starlight documentation under `/docs`) as one Astro project, deployed to GitHub Pages, with the public docs migrated off `docs/*.md` so the site is their single source.

**Architecture:** One Astro project in `site/`. A custom `src/pages/index.astro` owns `/` with full brand control. Starlight serves docs from `src/content/docs/docs/*` (its documented subpath pattern) at `/docs/*`. Both surfaces share one brand token sheet. Tooling lives in a devenv `site` profile using Bun. CI rewrites the existing `static.yml` to build and deploy `site/dist`.

**Tech Stack:** Astro 6, `@astrojs/starlight` (0.40+, which requires Astro 6), `@astrojs/sitemap`, `sharp`, `@fontsource-variable/inter`, `@fontsource-variable/jetbrains-mono`, `starlight-links-validator`. Bun as package manager and runner, node as build fallback. devenv 2.1.2 profiles. GitHub Actions + Pages.

**Source spec:** `docs/superpowers/specs/2026-06-15-right-agent-site-design.md`.

**Copy rules for every fresh string in this plan (landing copy, `concepts.mdx`, `commands.mdx`, docs index, the slimmed README):** apply `/writing-clearly-and-concisely` (active voice, positive form, concrete language, omit needless words, emphatic word last) and use no em-dashes (`—`). Match the established lowercase brand voice. Lead with the user-visible consequence.

**Verification model:** this is a static site, so there is no unit-test TDD loop. Each task ends with a concrete gate: `astro build`, `astro check`, link validation, or a grep assertion. Run the narrow gate per slice; run the full suite once at the end (Task 9). Site verification is separate from the Rust workspace: cargo and `enterTest` are never involved.

**Pre-flight (run once, do not skip):** confirm tool versions.

```bash
devenv version    # expect: devenv 2.1.x
git status        # expect: clean working tree on master (or your chosen branch)
```

---

## File map

Created under `site/` unless noted.

- `nix/site/devenv.nix`: devenv module defining the `site` profile (Bun, node, scripts).
- `devenv.yaml` (modify): add `imports: [ ./nix/site ]`.
- `site/package.json`, `site/bun.lock`, `site/tsconfig.json`, `site/.gitignore`.
- `site/astro.config.mjs`: site, base, Starlight, sitemap, links-validator.
- `site/src/content.config.ts`: Starlight docs collection.
- `site/src/styles/tokens.css`: brand custom properties (shared).
- `site/src/styles/fonts.css`: `@fontsource-variable` imports (shared).
- `site/src/styles/starlight.css`: maps brand tokens onto Starlight `--sl-*`.
- `site/src/styles/landing.css`: landing-only styles.
- `site/src/layouts/Landing.astro`: landing shell (head, fonts, footer slot).
- `site/src/components/{Hero,FeatureCard,CompareTable,Diagram,Roadmap,InstallBlock,Footer}.astro`.
- `site/src/pages/index.astro`: the landing, assembling the components.
- `site/src/content/docs/docs/{index,install,concepts,security,commands}.mdx`.
- `site/src/assets/`: copies of `lockup-horizontal.svg`, `mark-on-coal.svg`, `screenshot.png`.
- `.github/workflows/static.yml` (rewrite): Astro build and Pages deploy in devenv.
- `README.md` (rewrite, slim), `SECURITY.md` (create, 2-line pointer).
- Delete: `docs/INSTALL.md`, `docs/SECURITY.md`.

---

## Task 1: devenv `site` profile and Bun baseline

This is the de-risking foundation. Prove the profile activates and a minimal Astro+Starlight build is green under Bun before adding any content.

**Files:**
- Create: `nix/site/devenv.nix`
- Modify: `devenv.yaml`

- [ ] **Step 1: Write the devenv site profile module**

Create `nix/site/devenv.nix`:

```nix
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
```

- [ ] **Step 2: Import the module from devenv.yaml**

Edit `devenv.yaml`. Add a top-level `imports` key (place it above the `inputs:` block, after the schema comment):

```yaml
imports:
  - ./nix/site
```

- [ ] **Step 3: Verify the profile activates and exposes Bun**

Run:

```bash
devenv --profile site shell -- bash -lc 'bun --version && node --version'
```

Expected: prints a Bun version (for example `1.x.x`) then a Node version, no Nix evaluation error.

If this fails with a profile or import error, apply the fallback: delete `nix/site/devenv.nix` and the `imports` block, and instead add the same `profiles.site.module = { ... }` block directly inside the root `devenv.nix`. Re-run this step. The profile still keeps Bun out of the base shell.

- [ ] **Step 4: Scaffold the Astro project files**

Create the project skeleton.

`site/.gitignore`:

```gitignore
node_modules/
dist/
.astro/
```

`site/package.json`:

```json
{
  "name": "right-agent-site",
  "type": "module",
  "private": true,
  "scripts": {
    "dev": "astro dev",
    "build": "astro build",
    "preview": "astro preview",
    "check": "astro check"
  }
}
```

`site/tsconfig.json`:

```json
{
  "extends": "astro/tsconfigs/strict",
  "include": [".astro/types.d.ts", "**/*"],
  "exclude": ["dist"]
}
```

- [ ] **Step 5: Install dependencies with Bun (versions resolved at install, lockfile committed)**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && \
  bun add astro @astrojs/starlight @astrojs/sitemap sharp \
          @fontsource-variable/inter @fontsource-variable/jetbrains-mono && \
  bun add -d @astrojs/check typescript starlight-links-validator'
```

Expected: Bun writes resolved caret ranges into `site/package.json` and pins exact versions in `site/bun.lock`. No peer-dependency errors that abort install.

- [ ] **Step 6: Write the Astro config (minimal, build-able baseline)**

`site/astro.config.mjs`:

```js
// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';
import starlightLinksValidator from 'starlight-links-validator';

// GitHub Pages project page. Switch `site`/`base` (and add public/CNAME) for a custom domain.
export default defineConfig({
  site: 'https://onsails.github.io',
  base: '/right-agent',
  integrations: [
    starlight({
      title: 'right agent',
      // Docs live at src/content/docs/docs/* -> /docs/* (Starlight subpath pattern).
      customCss: [
        './src/styles/tokens.css',
        './src/styles/fonts.css',
        './src/styles/starlight.css',
      ],
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/onsails/right-agent' },
        { icon: 'telegram', label: 'Telegram', href: 'https://t.me/rightagent' },
      ],
      sidebar: [
        { label: 'Start', link: '/docs/' },
        { label: 'Install', link: '/docs/install/' },
        { label: 'Concepts', link: '/docs/concepts/' },
        { label: 'Security model', link: '/docs/security/' },
        { label: 'Telegram commands', link: '/docs/commands/' },
      ],
    }),
    sitemap(),
    starlightLinksValidator(),
  ],
});
```

- [ ] **Step 7: Write the Starlight content collection config**

`site/src/content.config.ts`:

```ts
import { defineCollection } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';

export const collections = {
  docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
};
```

- [ ] **Step 8: Add placeholder routes so the baseline build has a landing and a docs page**

`site/src/pages/index.astro`:

```astro
---
---
<!doctype html>
<html lang="en">
  <head><meta charset="utf-8" /><title>right agent</title></head>
  <body><h1>right agent</h1><p>baseline</p></body>
</html>
```

`site/src/content/docs/docs/index.mdx`:

```mdx
---
title: right agent docs
description: Documentation for right agent.
---

Baseline docs index.
```

- [ ] **Step 9: Create the three placeholder CSS files referenced by the config**

So the build does not fail on missing `customCss` files. Create empty-but-valid files; later tasks fill them.

```bash
mkdir -p site/src/styles
printf '/* brand tokens (Task 2) */\n' > site/src/styles/tokens.css
printf '/* fonts (Task 2) */\n' > site/src/styles/fonts.css
printf '/* starlight overrides (Task 2) */\n' > site/src/styles/starlight.css
```

- [ ] **Step 10: Run the baseline build (the gate)**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && bun run build && bun run check'
```

Expected: `astro build` completes, emits `site/dist/index.html` and `site/dist/docs/index.html`, the Pagefind index builds, links validation passes, and `astro check` reports 0 errors.

If `astro build` fails specifically inside `sharp`, re-run the build under node and record it: `bun install` then `node node_modules/astro/astro.js build`. Keep `bun run` for everything else.

- [ ] **Step 11: Commit**

```bash
git add nix/site/devenv.nix devenv.yaml site/.gitignore site/package.json \
        site/bun.lock site/tsconfig.json site/astro.config.mjs \
        site/src/content.config.ts site/src/pages/index.astro \
        site/src/content/docs/docs/index.mdx site/src/styles/
git commit -m "feat(site): scaffold Astro + Starlight under devenv site profile (Bun baseline)"
```

---

## Task 2: Brand tokens, fonts, and Starlight theme override

Define the coal-and-fire brand once. The landing and Starlight both consume it.

**Files:**
- Modify: `site/src/styles/tokens.css`, `site/src/styles/fonts.css`, `site/src/styles/starlight.css`

- [ ] **Step 1: Write the brand tokens**

Replace `site/src/styles/tokens.css`:

```css
/* Right Agent brand tokens. Source: docs/brand-guidelines.html */
:root {
  /* coal */
  --ra-coal-900: #0a0a0a;
  --ra-coal-800: #0f0f0f;
  --ra-coal-700: #161616;
  --ra-coal-600: #1a1a1a;

  /* fire (accent) */
  --ra-fire: #e8632a;
  --ra-fire-soft: #1a1108;

  /* cream / parchment */
  --ra-cream: #f2ede4;
  --ra-parchment: #ddd6c9;
  --ra-muted: #776e5e;

  /* semantic */
  --ra-red: #e03c3c;
  --ra-amber: #d9a82a;
  --ra-green: #6bbf59;
  --ra-blue: #4a90e2;

  --ra-font-sans: 'Inter Variable', system-ui, -apple-system, sans-serif;
  --ra-font-mono: 'JetBrains Mono Variable', ui-monospace, monospace;

  --ra-maxw: 72rem;
  --ra-radius: 10px;
}
```

- [ ] **Step 2: Write the font imports**

Replace `site/src/styles/fonts.css`:

```css
/* Self-hosted variable fonts (no Google CDN). Vite resolves these bare specifiers. */
@import '@fontsource-variable/inter';
@import '@fontsource-variable/jetbrains-mono';
```

- [ ] **Step 3: Map brand tokens onto Starlight variables (effectively dark-only coal)**

Replace `site/src/styles/starlight.css`:

```css
/* Force the coal-and-fire palette regardless of theme toggle. */
:root,
:root[data-theme='light'],
:root[data-theme='dark'] {
  --sl-font: var(--ra-font-sans);
  --sl-font-mono: var(--ra-font-mono);

  --sl-color-accent-low: var(--ra-fire-soft);
  --sl-color-accent: var(--ra-fire);
  --sl-color-accent-high: var(--ra-coal-900);

  --sl-color-white: var(--ra-cream);
  --sl-color-gray-1: var(--ra-cream);
  --sl-color-gray-2: var(--ra-parchment);
  --sl-color-gray-3: #b9b1a2;
  --sl-color-gray-4: var(--ra-muted);
  --sl-color-gray-5: #3a352d;
  --sl-color-gray-6: var(--ra-coal-700);
  --sl-color-gray-7: var(--ra-coal-600);
  --sl-color-black: var(--ra-coal-800);

  --sl-color-bg: var(--ra-coal-800);
  --sl-color-bg-nav: var(--ra-coal-700);
  --sl-color-bg-sidebar: var(--ra-coal-700);
  --sl-color-text-accent: var(--ra-fire);
}
```

- [ ] **Step 4: Rebuild to confirm tokens compile**

Run:

```bash
devenv --profile site shell -- bash -lc 'cd site && bun run build'
```

Expected: build succeeds, `site/dist/docs/index.html` now references the Inter/JetBrains font files and the coal palette CSS.

- [ ] **Step 5: Commit**

```bash
git add site/src/styles/
git commit -m "feat(site): coal-and-fire brand tokens, self-hosted fonts, Starlight theme map"
```

---

## Task 3: Copy brand assets into the site

Astro optimizes images it imports from `src/assets`. Copy the existing brand assets in.

**Files:**
- Create: `site/src/assets/lockup-horizontal.svg`, `site/src/assets/mark-on-coal.svg`, `site/src/assets/screenshot.png`

- [ ] **Step 1: Copy the assets**

```bash
mkdir -p site/src/assets
cp assets/lockup-horizontal.svg site/src/assets/lockup-horizontal.svg
cp assets/mark-on-coal.svg site/src/assets/mark-on-coal.svg
cp images/screenshot.png site/src/assets/screenshot.png
```

- [ ] **Step 2: Verify the files exist and are non-empty**

```bash
ls -l site/src/assets/
```

Expected: three files, each non-zero size.

- [ ] **Step 3: Commit**

```bash
git add site/src/assets/
git commit -m "feat(site): vendor brand lockup, mark, and screenshot into the site"
```

---

## Task 4: Migrate INSTALL and SECURITY into Starlight docs

Move the two public docs physically. Both contain only external links, so the move needs Starlight frontmatter, nothing else rewritten inside them.

**Files:**
- Move: `docs/INSTALL.md` -> `site/src/content/docs/docs/install.mdx`
- Move: `docs/SECURITY.md` -> `site/src/content/docs/docs/security.mdx`

- [ ] **Step 1: Move both files with git (preserves history)**

```bash
git mv docs/INSTALL.md site/src/content/docs/docs/install.mdx
git mv docs/SECURITY.md site/src/content/docs/docs/security.mdx
```

- [ ] **Step 2: Add frontmatter and drop the now-redundant H1 in install.mdx**

Open `site/src/content/docs/docs/install.mdx`. Starlight renders the `title` as the page H1, so remove the leading `# Installation` line and prepend frontmatter. The file must begin with:

```mdx
---
title: Install
description: Prerequisites, install paths, and first-run setup for right agent.
---
```

Delete the old `# Installation` heading line that followed it. Leave the rest of the body unchanged.

- [ ] **Step 3: Add frontmatter and drop the H1 in security.mdx**

Open `site/src/content/docs/docs/security.mdx`. Prepend:

```mdx
---
title: Security model
description: The sandbox, credential, and network model behind right agent.
---
```

Delete the old `# Security Model` heading line. Leave the rest unchanged.

- [ ] **Step 4: Build to confirm both pages render at the expected URLs**

```bash
devenv --profile site shell -- bash -lc 'cd site && bun run build'
test -f site/dist/docs/install/index.html && test -f site/dist/docs/security/index.html && echo OK
```

Expected: prints `OK`. Pages exist at `/docs/install` and `/docs/security`.

- [ ] **Step 5: Confirm the originals are gone**

```bash
test ! -e docs/INSTALL.md && test ! -e docs/SECURITY.md && echo "originals removed"
```

Expected: prints `originals removed`. (README link rewrites happen in Task 7.)

- [ ] **Step 6: Commit**

```bash
git add site/src/content/docs/docs/install.mdx site/src/content/docs/docs/security.mdx docs/
git commit -m "feat(site): migrate INSTALL and SECURITY into Starlight docs (single source)"
```

---

## Task 5: Author the fresh docs pages (index, concepts, commands)

New content, no original to move. Drawn from the README, rewritten concisely, no em-dashes.

**Files:**
- Modify: `site/src/content/docs/docs/index.mdx`
- Create: `site/src/content/docs/docs/concepts.mdx`, `site/src/content/docs/docs/commands.mdx`

- [ ] **Step 1: Write the docs landing**

Replace `site/src/content/docs/docs/index.mdx`:

```mdx
---
title: right agent docs
description: How to install, run, and reason about right agent.
---

right agent is an ai agent you run by messaging it. you give it real credentials
without handing them to the model: every agent runs in its own sandbox, and every
credential lives outside it.

Start here:

- [Install](/docs/install/): prerequisites and first-run setup.
- [Concepts](/docs/concepts/): how agents, sandboxes, memory, and skills fit together.
- [Security model](/docs/security/): the sandbox, credential, and network model.
- [Telegram commands](/docs/commands/): the slash commands you use day to day.
```

- [ ] **Step 2: Write the concepts page**

Create `site/src/content/docs/docs/concepts.mdx`:

```mdx
---
title: Concepts
description: How agents, sandboxes, memory, and learned skills fit together.
---

## One bot per agent

You talk to an agent in Telegram. Each surface, a dm, a group, or a forum topic
inside a group, is its own Claude Code session, keyed by chat and thread. All
sessions share one chat-tagged memory, so separate working contexts still
remember the same things about you. In groups the agent stays quiet until you
@mention or reply to it.

## Every agent in its own sandbox

Each agent gets a persistent OpenShell sandbox with its own filesystem
(landlock), its own scoped network, and a tls-terminating proxy. A misbehaving
agent cannot reach the host, the other agents, or arbitrary networks. Sandboxes
persist: they live as long as the agent and survive restarts.

## Credentials stay outside the box

mcp tokens and provider keys live on the host. The sandbox sees opaque
placeholders that the outbound proxy substitutes on each request. The secret
bytes never enter the sandbox, and the proxy never resolves them onto the open
internet. A compromised agent can misuse a tool while it runs, but it cannot read
the credential.

## Memory it can trust

Memory persists across sessions and chats. It runs on Hindsight by default, with
an agent-managed `MEMORY.md` file mode as fallback. Recall shows when each memory
formed and lets the model judge relevance, with no hidden staleness thresholds.
Memory is treated as untrusted data: incoming memories are sanitized, and
recalled memory is framed so the model weighs it as information, not commands.

## Skills it learns on its own

When the agent works something out during real use, an api quirk or a multi-step
workflow, a per-turn pipeline captures it into a reusable skill and loads it in
later sessions. The platform records what each skill costs and how often it runs,
a curator prunes the ones that do not earn their keep, and you pin or unpin
skills from the dashboard.

## A dashboard inside Telegram

`/mcp` and `/providers` open a Mini App dashboard with views for health,
activity, identity, learned skills, and usage with cost. All management is
proxied to the bot's control plane, and credentials you enter are accepted but
never displayed. You never hand-edit config or credential files.
```

- [ ] **Step 3: Write the commands page**

Create `site/src/content/docs/docs/commands.mdx`:

```mdx
---
title: Telegram commands
description: The slash commands you use to run an agent from Telegram.
---

You run an agent from chat, not a terminal. These are the commands you reach for.

| Command | What it does |
| --- | --- |
| `/start` | Start talking to the agent. |
| `/new <name>` | Start a fresh named session in this chat or topic. |
| `/list` | Show this chat's sessions. |
| `/switch <id>` | Move between sessions. |
| `/model` | Switch the Claude model from an inline menu. Hot-reloads, no restart. |
| `/debug [on\|off\|status]` | Toggle debug mode for the next invocations. |
| `/doctor` | Report agent and sandbox health in chat. |
| `/cron [list\|<id>]` | Show scheduled-job status. Creation is in the dashboard. |
| `/dashboard` | Open the full Mini App dashboard. |
| `/mcp` | Open the dashboard mcp view. |
| `/providers` | Open the dashboard providers view. |
| `/allow`, `/deny`, `/allowed`, `/allow_all`, `/deny_all` | Manage who the agent will talk to. |
```

- [ ] **Step 4: Build and validate links**

```bash
devenv --profile site shell -- bash -lc 'cd site && bun run build'
```

Expected: build succeeds. `starlight-links-validator` passes, confirming the internal `/docs/*` links in these pages resolve.

- [ ] **Step 5: Commit**

```bash
git add site/src/content/docs/docs/
git commit -m "docs(site): author docs index, concepts, and commands pages"
```

---

## Task 6: Landing components and page

Build the landing at `/`. Components are functional and token-driven. Visual polish is iterative (out of scope for v1 per the spec).

**Files:**
- Create: `site/src/styles/landing.css`, `site/src/layouts/Landing.astro`, the seven components, and `site/src/pages/index.astro` (replacing the placeholder).

- [ ] **Step 1: Write the landing stylesheet**

Create `site/src/styles/landing.css`:

```css
@import './tokens.css';
@import './fonts.css';

* { box-sizing: border-box; }
html { scroll-behavior: smooth; }
body {
  margin: 0;
  background: var(--ra-coal-800);
  color: var(--ra-cream);
  font-family: var(--ra-font-sans);
  line-height: 1.6;
}
a { color: var(--ra-fire); text-decoration: none; }
a:hover { text-decoration: underline; }
code, pre { font-family: var(--ra-font-mono); }

.ra-wrap { max-width: var(--ra-maxw); margin: 0 auto; padding: 0 1.25rem; }
.ra-section { padding: 5rem 0; border-top: 1px solid var(--ra-coal-600); }
.ra-section h2 { font-size: 1.9rem; margin: 0 0 2rem; }
.ra-grid { display: grid; gap: 1.25rem; grid-template-columns: repeat(auto-fit, minmax(16rem, 1fr)); }
.ra-card {
  background: var(--ra-coal-700);
  border: 1px solid var(--ra-coal-600);
  border-radius: var(--ra-radius);
  padding: 1.5rem;
}
.ra-card h3 { margin: 0 0 0.5rem; color: var(--ra-cream); font-size: 1.1rem; }
.ra-card p { margin: 0; color: var(--ra-parchment); }
.ra-btn {
  display: inline-block;
  background: var(--ra-fire);
  color: var(--ra-coal-900);
  font-weight: 600;
  padding: 0.7rem 1.3rem;
  border-radius: var(--ra-radius);
}
.ra-btn:hover { text-decoration: none; filter: brightness(1.08); }
table.ra-compare { width: 100%; border-collapse: collapse; }
.ra-compare th, .ra-compare td { padding: 0.7rem; border-bottom: 1px solid var(--ra-coal-600); text-align: left; }
.ra-compare th { color: var(--ra-fire); }
```

- [ ] **Step 2: Write the landing layout**

Create `site/src/layouts/Landing.astro`:

```astro
---
import '../styles/landing.css';
interface Props { title?: string; description?: string; }
const {
  title = 'right agent',
  description = 'an ai agent you run by messaging it. credentials stay outside the box.',
} = Astro.props;
---
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{title}</title>
    <meta name="description" content={description} />
    <link rel="icon" href={`${import.meta.env.BASE_URL}/favicon.svg`} />
  </head>
  <body>
    <slot />
  </body>
</html>
```

- [ ] **Step 3: Write the Hero component**

Create `site/src/components/Hero.astro`:

```astro
---
import { Image } from 'astro:assets';
import lockup from '../assets/lockup-horizontal.svg';
import screenshot from '../assets/screenshot.png';
---
<header class="ra-section" style="border-top:none; padding-top:3rem;">
  <div class="ra-wrap">
    <Image src={lockup} alt="right agent" height={28} />
    <h1 style="font-size:2.6rem; line-height:1.2; margin:1.5rem 0 1rem; max-width:34ch;">
      an ai agent you run by messaging it
    </h1>
    <p style="font-size:1.15rem; color:var(--ra-parchment); max-width:60ch;">
      you can give it real credentials without handing them to the model. every
      agent runs in its own sandbox, every credential lives outside it. the box is
      closed; you just use it.
    </p>
    <p style="margin:1.75rem 0;">
      <a class="ra-btn" href="#install">install</a>
      <a href={`${import.meta.env.BASE_URL}/docs/`} style="margin-left:1rem;">read the docs</a>
    </p>
    <p style="font-size:0.9rem; color:var(--ra-muted);">
      today every agent runs on Claude Code, so you need a Claude subscription.
    </p>
    <Image src={screenshot} alt="right agent in Telegram" width={900}
           style="width:100%; max-width:720px; margin-top:2rem; border:1px solid var(--ra-coal-600); border-radius:var(--ra-radius);" />
  </div>
</header>
```

- [ ] **Step 4: Write the FeatureCard component**

Create `site/src/components/FeatureCard.astro`:

```astro
---
interface Props { title: string; }
const { title } = Astro.props;
---
<article class="ra-card">
  <h3>{title}</h3>
  <p><slot /></p>
</article>
```

- [ ] **Step 5: Write the CompareTable component**

Create `site/src/components/CompareTable.astro`:

```astro
---
const rows = [
  ['setup', 'wire the stack yourself over a weekend', 'curl installer, right init, right up'],
  ['daily use', 'a service you operate from the cli', 'a bot you message in Telegram'],
  ['credentials', 'given to the agent', 'held on the host, injected at the proxy'],
  ['isolation', 'opt-in, often skipped', 'per-agent sandbox by default'],
  ['on failure', 'may fall back to a looser path', 'fails closed, diagnoses, retries'],
  ['memory', 'replay the full history each turn', 'persistent, dated, model-judged recall'],
  ['cost', 'often per-agent', 'many agents, one Claude subscription'],
  ['recovery', 'manual fixes, often from scratch', 'self-heals; data is preserved'],
];
---
<table class="ra-compare">
  <thead><tr><th></th><th>typical agent setup</th><th>right agent</th></tr></thead>
  <tbody>
    {rows.map(([k, a, b]) => (<tr><td><strong>{k}</strong></td><td>{a}</td><td>{b}</td></tr>))}
  </tbody>
</table>
```

- [ ] **Step 6: Write the Diagram component (static, brand-styled, no Mermaid)**

Create `site/src/components/Diagram.astro`:

```astro
---
const tiers = [
  { label: 'Cloud', items: ['Telegram', 'Cloudflare', 'Anthropic', 'Hindsight', 'Linear / Notion / Gmail'] },
  { label: 'Host', items: ['Bot per agent', 'MCP aggregator (holds credentials)', 'OpenShell gateway', 'cloudflared'] },
  { label: 'Sandbox per agent', items: ['Claude Code', 'Identity', 'scoped fs + network + tls proxy'] },
];
---
<div style="display:grid; gap:1rem;">
  {tiers.map((t) => (
    <div class="ra-card">
      <h3 style="color:var(--ra-fire);">{t.label}</h3>
      <p style="color:var(--ra-parchment);">{t.items.join('  ·  ')}</p>
    </div>
  ))}
</div>
```

- [ ] **Step 7: Write the Roadmap component**

Create `site/src/components/Roadmap.astro`:

```astro
---
const shipped = [
  'multi-agent orchestration, sandboxed by default',
  'live Telegram Mini App dashboard with usage and cost',
  'mcp aggregator with auto-detected oauth, bearer, header, query-string auth',
  'credential providers injected at the outbound proxy',
  'automatic skill learning with curator pruning',
  'fail-closed sandbox with a self-healing supervisor',
];
const next = [
  'zero-token clis: aws, gcloud, kubectl',
  'native browser automation',
  'shareable agent templates',
  'agent-to-agent communication',
];
---
<div class="ra-grid">
  <div class="ra-card">
    <h3>shipped</h3>
    <ul style="margin:0; padding-left:1.1rem; color:var(--ra-parchment);">
      {shipped.map((i) => <li>{i}</li>)}
    </ul>
  </div>
  <div class="ra-card">
    <h3>next</h3>
    <ul style="margin:0; padding-left:1.1rem; color:var(--ra-parchment);">
      {next.map((i) => <li>{i}</li>)}
    </ul>
  </div>
</div>
```

- [ ] **Step 8: Write the InstallBlock component (with copy-to-clipboard)**

Create `site/src/components/InstallBlock.astro`:

```astro
---
const cmd = 'curl -LsSf https://raw.githubusercontent.com/onsails/right-agent/master/install.sh | sh';
---
<div class="ra-card" style="display:flex; gap:1rem; align-items:center; justify-content:space-between; flex-wrap:wrap;">
  <code id="ra-install-cmd" style="overflow-x:auto;">{cmd}</code>
  <button id="ra-copy" class="ra-btn" type="button" style="border:none; cursor:pointer;">copy</button>
</div>
<p style="color:var(--ra-parchment);">then open a new shell and run <code>right up</code>, then message your bot.</p>
<script>
  const btn = document.getElementById('ra-copy');
  const src = document.getElementById('ra-install-cmd');
  btn?.addEventListener('click', async () => {
    await navigator.clipboard.writeText(src?.textContent ?? '');
    btn.textContent = 'copied';
    setTimeout(() => (btn.textContent = 'copy'), 1500);
  });
</script>
```

- [ ] **Step 9: Write the Footer component**

Create `site/src/components/Footer.astro`:

```astro
<footer class="ra-section">
  <div class="ra-wrap" style="color:var(--ra-muted); display:flex; gap:1.5rem; flex-wrap:wrap;">
    <a href="https://github.com/onsails/right-agent">GitHub</a>
    <a href="https://t.me/rightagent">Telegram</a>
    <a href={`${import.meta.env.BASE_URL}/docs/`}>Docs</a>
    <span>built on Claude Code, NVIDIA OpenShell, process-compose. Apache-2.0.</span>
  </div>
</footer>
```

- [ ] **Step 10: Assemble the landing page**

Replace `site/src/pages/index.astro`:

```astro
---
import Landing from '../layouts/Landing.astro';
import Hero from '../components/Hero.astro';
import FeatureCard from '../components/FeatureCard.astro';
import CompareTable from '../components/CompareTable.astro';
import Diagram from '../components/Diagram.astro';
import Roadmap from '../components/Roadmap.astro';
import InstallBlock from '../components/InstallBlock.astro';
import Footer from '../components/Footer.astro';
---
<Landing>
  <Hero />

  <section class="ra-section"><div class="ra-wrap">
    <h2>what you get</h2>
    <div class="ra-grid">
      <FeatureCard title="credentials stay outside the box">the secret bytes never enter the sandbox; the proxy substitutes them on outbound requests.</FeatureCard>
      <FeatureCard title="every agent in its own sandbox">scoped filesystem, network, and a tls-terminating proxy per agent.</FeatureCard>
      <FeatureCard title="memory is untrusted data">incoming memory is sanitized; recalled memory is framed as information, not commands.</FeatureCard>
      <FeatureCard title="skills it learns on its own">reusable skills captured from real use, pruned by a curator, pinned from the dashboard.</FeatureCard>
      <FeatureCard title="one bot per agent">message it like a person; each chat is its own session over one shared memory.</FeatureCard>
      <FeatureCard title="many agents, one subscription">cost scales with subscriptions, not agent count.</FeatureCard>
    </div>
  </div></section>

  <section class="ra-section"><div class="ra-wrap">
    <h2>how it works</h2>
    <p style="color:var(--ra-parchment); max-width:65ch;">a message reaches the host through Cloudflare, the bot routes it to that chat's Claude Code session inside the agent's sandbox, and replies. credentials are injected at the proxy on the way out, never inside the box.</p>
    <div style="margin-top:2rem;"><Diagram /></div>
  </div></section>

  <section class="ra-section"><div class="ra-wrap">
    <h2>how it compares</h2>
    <CompareTable />
  </div></section>

  <section class="ra-section" id="security"><div class="ra-wrap">
    <h2>security by default</h2>
    <p style="color:var(--ra-parchment); max-width:65ch;">sandboxed by default, credentials on the host, fails closed on a backend outage. read the full <a href={`${import.meta.env.BASE_URL}/docs/security/`}>security model</a>.</p>
  </div></section>

  <section class="ra-section"><div class="ra-wrap">
    <h2>roadmap</h2>
    <Roadmap />
  </div></section>

  <section class="ra-section" id="install"><div class="ra-wrap">
    <h2>install</h2>
    <InstallBlock />
  </div></section>

  <Footer />
</Landing>
```

- [ ] **Step 11: Add a favicon so the layout link resolves**

```bash
cp assets/mark-on-coal.svg site/public/favicon.svg
```

(Create `site/public/` if it does not exist.)

- [ ] **Step 12: Build and eyeball locally**

```bash
devenv --profile site shell -- bash -lc 'cd site && bun run build'
```

Expected: build succeeds, links validation passes. Optionally run `bun run preview` and open the printed URL to eyeball the landing.

- [ ] **Step 13: Commit**

```bash
git add site/src/styles/landing.css site/src/layouts/ site/src/components/ \
        site/src/pages/index.astro site/public/
git commit -m "feat(site): brand landing page with hero, features, compare, roadmap, install"
```

---

## Task 7: Slim the README and rewrite the migrated links

The README becomes a short entry point styled after Hermes (lockup, badges, hero, screenshot, quick start, docs link). The full narrative now lives on the site, so the two do not duplicate each other.

**Files:**
- Rewrite: `README.md`
- Create: `SECURITY.md` (root, 2-line pointer)

- [ ] **Step 1: Replace README.md**

Replace the entire `README.md` with:

```markdown
<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-E8632A.svg" alt="license"></a>
  <a href="https://github.com/onsails/right-agent/actions"><img src="https://github.com/onsails/right-agent/actions/workflows/build.yml/badge.svg" alt="build"></a>
  <a href="https://t.me/rightagent"><img src="https://img.shields.io/badge/Telegram-chat-E8632A?logo=telegram" alt="telegram"></a>
</p>

<p align="center">
  <img src="assets/lockup-horizontal.svg" height="36" alt="right agent">
</p>

right agent is an ai agent you run by messaging it. you give it real credentials
without handing them to the model: every agent runs in its own sandbox, every
credential lives outside it. the secret bytes never enter the box, so a
compromised agent can misuse a tool while it runs, but it cannot read the
credential or reach the open internet with it. for anyone tired of "grant all
permissions and hope," that is the change.

<p align="center">
  <img src="images/screenshot.png" alt="right agent in Telegram" width="720">
</p>

> today every agent runs on Claude Code (`claude -p`), so you need a Claude
> subscription. multi-provider support is in the works.

## quick start

```sh
curl -LsSf https://raw.githubusercontent.com/onsails/right-agent/master/install.sh | sh
```

open a new shell so `right` is on your `PATH`, then:

```sh
right up
```

message your bot on Telegram. the first chat walks you through login. from there
you manage everything from Telegram: `/mcp` and `/providers` open the dashboard.

## docs

full story, install guide, security model, concepts, and commands:
**https://onsails.github.io/right-agent/**

- [Install](https://onsails.github.io/right-agent/docs/install/)
- [Concepts](https://onsails.github.io/right-agent/docs/concepts/)
- [Security model](https://onsails.github.io/right-agent/docs/security/)
- [Telegram commands](https://onsails.github.io/right-agent/docs/commands/)

Contributor docs stay in the repo: [ARCHITECTURE.md](ARCHITECTURE.md),
[PROMPT_SYSTEM.md](PROMPT_SYSTEM.md).

## credits

built on [Claude Code](https://docs.anthropic.com/en/docs/claude-code),
[NVIDIA OpenShell](https://github.com/NVIDIA/OpenShell), and
[process-compose](https://github.com/F1bonacc1/process-compose). licensed under
Apache-2.0.
```

- [ ] **Step 2: Create the root SECURITY.md pointer (preserves the GitHub Security tab)**

Create `SECURITY.md`:

```markdown
# Security

The security model lives on the site:
https://onsails.github.io/right-agent/docs/security/
```

- [ ] **Step 3: Confirm no source file still points at the deleted docs**

```bash
rg -n 'docs/INSTALL\.md|docs/SECURITY\.md' -g '!docs/superpowers/**' -g '!docs/harness-migration-research.md' -g '!docs/plans/**' || echo "no dangling refs"
```

Expected: prints `no dangling refs`. (The excluded files reference unrelated or historical content, verified during planning.)

- [ ] **Step 4: Commit**

```bash
git add README.md SECURITY.md
git commit -m "docs: slim README to entry point, point docs at the site, add Security pointer"
```

---

## Task 8: Rewrite the Pages deploy workflow

Replace the vestigial `static.yml` (it publishes the whole repo) with an Astro build and deploy that runs inside devenv with the `site` profile. Do not add a second Pages workflow; two would fight over the `pages` concurrency group.

**Files:**
- Rewrite: `.github/workflows/static.yml`

- [ ] **Step 1: Replace the workflow**

Replace `.github/workflows/static.yml` with:

```yaml
# Build the Astro site and deploy it to GitHub Pages.
name: Deploy site to Pages

on:
  push:
    branches: ["master"]
    paths:
      - "site/**"
      - "nix/site/**"
      - ".github/workflows/static.yml"
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: "pages"
  cancel-in-progress: false

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          lfs: true
      # Determinate Nix + devenv + FlakeHub cache, matching tests.yml.
      - uses: onsails/nix-action@v2.2.0
      - name: Build site (Bun, in the devenv site profile)
        run: |
          devenv --profile site shell -- bash -lc \
            'cd site && bun install --frozen-lockfile && bun run build'
      - name: Upload Pages artifact
        uses: actions/upload-pages-artifact@v5
        with:
          path: site/dist
      - name: Setup Pages
        uses: actions/configure-pages@v6

  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - name: Deploy to GitHub Pages
        id: deployment
        uses: actions/deploy-pages@v5
```

- [ ] **Step 2: Lint the workflow**

```bash
devenv shell -- actionlint .github/workflows/static.yml
```

Expected: no errors. (`actionlint` is in the base devenv.)

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/static.yml
git commit -m "ci(site): build and deploy Astro site to Pages via devenv site profile"
```

---

## Task 9: Final verification and handoff

Run the full site gate once from a clean state. This is the mandatory end check.

- [ ] **Step 1: Clean build, check, and link validation**

```bash
devenv --profile site shell -- bash -lc 'cd site && rm -rf dist .astro && bun run check && bun run build'
```

Expected: `astro check` reports 0 errors. `astro build` succeeds, `starlight-links-validator` passes, and the Pagefind index builds. `site/dist` contains `index.html`, `docs/index.html`, `docs/install/index.html`, `docs/concepts/index.html`, `docs/security/index.html`, `docs/commands/index.html`.

- [ ] **Step 2: Confirm base-path correctness in the output**

```bash
rg -o 'href="/right-agent/docs/[a-z]+/"' site/dist/index.html | head
```

Expected: landing links carry the `/right-agent` base. (Astro `import.meta.env.BASE_URL` and `Image`/Starlight emit base-aware URLs. If any landing link is missing the base, replace the hardcoded path with a `import.meta.env.BASE_URL`-prefixed one.)

- [ ] **Step 3: Confirm the profile leaves the base shell lean**

```bash
devenv shell -- bash -lc 'command -v bun && echo "LEAK: bun in base shell" || echo "base shell clean"'
```

Expected: prints `base shell clean` (Bun appears only under `--profile site`).

- [ ] **Step 4: Manual GitHub step (cannot be scripted)**

In the repo on GitHub: Settings -> Pages -> Build and deployment -> Source = "GitHub Actions". Note this in the PR description so the maintainer flips it before the first deploy. Without it the workflow runs but Pages serves nothing.

- [ ] **Step 5: Final commit if anything changed**

```bash
git status
# commit only if Step 2 required a base-path fix:
# git add site/ && git commit -m "fix(site): base-path-correct internal links"
```

---

## Self-review checklist (completed during planning)

- **Spec coverage:** framework (Task 1), devenv profile + Bun (Task 1), brand tokens + fonts (Task 2), assets (Task 3), docs migration with single-source deletion (Task 4), fresh docs (Task 5), landing sections all eight (Task 6), README slim + Security pointer + link rewrites (Task 7), CI replacing static.yml (Task 8), final verification + base-path + profile sanity (Task 9). All spec sections map to a task.
- **No placeholders:** every code step shows complete file content or an exact command with expected output. Migrated docs use `git mv` plus shown frontmatter; their bodies are preserved from source, not re-authored.
- **Type/name consistency:** component names match between `index.astro` imports and the component files; CSS class names (`ra-card`, `ra-section`, `ra-wrap`, `ra-grid`, `ra-btn`, `ra-compare`) are defined in `landing.css` and used consistently; `--ra-*` and `--sl-*` variable names match across `tokens.css` and `starlight.css`.
- **Open verification risks flagged in-line:** Bun `sharp` fallback to node (Task 1 Step 10), imported-profile fallback to root `devenv.nix` (Task 1 Step 3), base-path correctness (Task 9 Step 2), manual Pages source toggle (Task 9 Step 4).
