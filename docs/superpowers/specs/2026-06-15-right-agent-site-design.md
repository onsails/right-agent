# Right Agent Site: Design Spec

**Date:** 2026-06-15
**Status:** Approved for planning
**Topic:** Public website (marketing landing + documentation) for Right Agent

## Goal

Ship a public website in one build pass, then improve it iteratively. The site
does two jobs: a brand-forward marketing landing that sells the product and
drives users into install, and a navigable documentation section with sidebar
and search. The site becomes the single home for the full product narrative and
for the public docs we migrate into it.

The first pass produces a complete, on-brand, deployable site. Rich
interactivity (scroll-driven animation, live demos) comes in later iterations,
not in v1.

## Decisions (settled during brainstorming)

- **Framework:** bare Astro for the landing, Starlight mounted at `/docs` for the
  documentation. One Astro project, one JS toolchain.
- **Rejected alternatives:** Starlight-only (its docs theme fights a highly
  custom brand on the landing) and Zensical-all-in (landing lives in MiniJinja
  templates, the most brand-critical surface becomes the weaker half, and it
  pulls in a separate Python+Rust toolchain). The deciding factor: the more
  custom the brand, the less a docs theme's built-in aesthetic helps, and the
  more a component model pays off. Right Agent's brand is highly custom.
- **Hosting:** GitHub Pages, deployed by a GitHub Actions workflow.
- **URL:** project page `https://onsails.github.io/right-agent/`, so Astro runs
  with `base: '/right-agent'`. A future custom domain is a one-line config change
  plus a `CNAME`.
- **Source of truth:** migrated docs live only on the site. No duplicate `.md`
  files. Originals are deleted and every reference to them is rewritten.
- **Interactivity level for v1:** Static+ (full brand, dark theme, hover states,
  copy-to-clipboard on install commands, a CSS terminal frame around commands).
  No typing effects or scroll animations in v1.

## Non-goals (v1)

- No port of contributor documentation (`ARCHITECTURE.md`, `docs/architecture/*`,
  `PROMPT_SYSTEM.md`). These stay in the repo for contributors and agent context.
- No build-time import of README content into the landing. The landing is
  authored fresh; the README is slimmed so the full narrative lives in one place.
- No rich interactivity, no i18n, no versioned docs, no blog. These are later
  iterations.
- No custom domain in v1.

## Architecture

One Astro project in `site/`. A custom `src/pages/index.astro` owns the root
route and renders the marketing landing with full brand control. Starlight, added
as an Astro integration, generates the documentation routes under `/docs` from a
content collection. The two surfaces share one brand token sheet, so they read as
one product.

### Repo layout

```
site/
  astro.config.mjs          # site + base, starlight() integration, sitemap
  package.json
  pnpm-lock.yaml
  tsconfig.json
  public/                   # favicon, og-image (CNAME later, if a domain is added)
  src/
    assets/                 # screenshot.png, lockup/marks (Astro optimizes these)
    styles/
      tokens.css            # brand custom properties (coal + fire, fonts), SHARED
      landing.css           # landing-only styles
    components/             # landing components (.astro):
                            #   Hero, FeatureCard, CompareTable, Diagram,
                            #   Roadmap, InstallBlock, Footer
    layouts/
      Landing.astro
    pages/
      index.astro           # the marketing landing (custom, full brand)
    content/
      docs/                 # Starlight content collection (MDX)
        install.mdx         # moved from docs/INSTALL.md
        concepts.mdx        # new (public version of how-it-works)
        security.mdx        # moved from docs/SECURITY.md
        commands.mdx        # new (slash-command reference)
    content.config.ts       # Starlight docs collection schema
.github/workflows/site.yml  # build and deploy to GitHub Pages (separate from build.yml)
```

The Rust `build.yml` workflow is untouched. The site gets its own `site.yml`.

## Content scope

### Landing `/` (section order)

1. **Hero**: tagline, Telegram screenshot, install CTA, a short note that a
   Claude subscription is needed today.
2. **What you get**: six strongest value cards: credentials stay outside the
   box; per-agent sandbox; memory treated as untrusted data; self-learned skills;
   one bot per agent; many agents on one Claude subscription.
3. **How it works**: short narrative plus the architecture diagram (the same
   one the README ships as Mermaid).
4. **Living in Telegram**: the chat-first experience: per-surface sessions,
   attachments, voice, the Mini App dashboard, slash commands.
5. **How it compares**: the "typical setup vs right agent" table.
6. **Security**: a tight summary with a link to `/docs/security`.
7. **Roadmap**: shipped and next.
8. **Footer / install CTA**: the install one-liner, links to GitHub, Telegram,
   and the docs.

### Docs `/docs` (v1, deliberately lean)

- **Getting started / Install**: migrated from `docs/INSTALL.md` plus the
  README install block.
- **Concepts / How it works**: new, public version: agents, sandbox, memory,
  skills, dashboard. Not the prescriptive `ARCHITECTURE.md`.
- **Security model**: migrated from `docs/SECURITY.md`.
- **Telegram commands**: new, the slash-command reference drawn from the README.

## Migration (single source of truth)

Verified blast radius is small. Every reference below was found by grep and must
be rewritten so nothing points at a deleted or duplicated file.

**Physically move, delete the original, rewrite references:**

| Original | New home | References to rewrite |
|---|---|---|
| `docs/INSTALL.md` | `site/src/content/docs/install.mdx` | `README.md:184`, `README.md:247` |
| `docs/SECURITY.md` | `site/src/content/docs/security.mdx` | `README.md:200`, `README.md:248` |

Rewrite targets point at the published site URLs
(`https://onsails.github.io/right-agent/docs/install` and `.../docs/security`).

**Authored fresh on the site (no original to move):** `concepts.mdx`,
`commands.mdx`.

**Stay in the repo, untouched (contributor / agent-context, not website
content):** `ARCHITECTURE.md`, `AGENTS.md`, `AGENTS.rust.md`,
`docs/architecture/*`, `PROMPT_SYSTEM.md`, `docs/superpowers/*`.
`README.md:249` keeps its link to `ARCHITECTURE.md` as a GitHub file link for
contributors, since that doc does not move.

**GitHub Security policy tab:** moving `docs/SECURITY.md` would remove GitHub's
"Security policy" tab. Preserve it with a two-line `SECURITY.md` at the repo root
that links to `/docs/security`. This is a stable pointer, not duplicated content.

## README v2 (slim)

The README slims to a short entry point, styled after the Hermes Agent README
(centered lockup, badges, hero tagline, screenshot) but cut to three blocks. The
full product narrative then lives only on the site, so the landing and README do
not duplicate each other.

1. Centered lockup (`assets/lockup-horizontal.svg`) plus the existing badges
   (license, build, Telegram).
2. Hero tagline: a short "who we are, what we are about" paragraph in the
   established lowercase voice, condensed from the current README opening.
3. Screenshot (`images/screenshot.png`).
4. **Quick start:** `curl ... | sh`, then `right up`, then "message your bot."
5. **Docs link** to the website for the full story, install, security, concepts,
   and commands.
6. Footer: credits and Apache-2.0.

## Design system

`tokens.css` defines the brand as CSS custom properties, taken from
`docs/brand-guidelines.html`:

- **Coal:** `#0f0f0f`, `#161616`, `#0a0a0a`.
- **Fire (accent):** `#E8632A`.
- **Cream / parchment:** `#f2ede4`, `#ddd6c9`.
- **Semantic:** red `#e03c3c`, amber `#d9a82a`, green `#6bbf59`, blue `#4a90e2`.
- **Fonts:** Inter (sans) and JetBrains Mono (mono), self-hosted via `@fontsource`
  packages so the site carries no Google Fonts CDN dependency.

Starlight receives the same `tokens.css` through its `customCss` option, which
overrides its `--sl-*` variables to the coal-and-fire palette. The landing
imports the same file. One brand definition feeds both surfaces.

Existing assets are used as-is: `assets/lockup-horizontal.svg`,
`assets/mark-on-coal.svg`, `assets/character-on-coal.svg`, `images/screenshot.png`.

## Copy guidelines

All newly written copy (landing sections, `concepts.mdx`, `commands.mdx`, the
slimmed README, doc intros) follows two rules:

1. **Run it through the `/writing-clearly-and-concisely` skill**
   (`elements-of-style:writing-clearly-and-concisely`). Apply active voice,
   positive form, concrete language, omit needless words, keep related words
   together, and place emphatic words at the end.
2. **No em-dashes (`—`).** Restructure the sentence, or use a colon, comma,
   period, or parentheses instead.

Match the established brand voice: lowercase, direct, concrete, the "closed box"
positioning. Lead with the user-visible consequence, not the mechanism. Verify
claims against the code and docs before stating them. Migrated docs keep their
substance; only reformat them into MDX and fix links, do not rewrite their
meaning.

## Build, deploy, verification

- **Package manager:** pnpm.
- **Deploy workflow (`site.yml`):** trigger on push to `master` touching
  `site/**`; set up node and pnpm; run `astro build` with the configured `base`;
  upload `site/dist` with `actions/upload-pages-artifact`; deploy with
  `actions/deploy-pages`. Set the GitHub Pages source to GitHub Actions.
- **Verification is the site's own, separate from the Rust workspace** (the site
  touches no Rust crates, so cargo is not involved):
  - `astro build` succeeds.
  - `astro check` passes (types).
  - Broken-link validation across the build output (for example the
    `starlight-links-validator` plugin, plus a link check over `dist` for the
    landing), so the rewritten README and cross-page links resolve.
  - Starlight's Pagefind search index builds.
  - Lighthouse and visual polish are manual and iterative.

## Defaults accepted (switchable later)

- No custom domain: `base: '/right-agent'`. Adding a domain is one config line
  plus a `CNAME`.
- GitHub Security tab preserved by a two-line root `SECURITY.md` pointer.
- pnpm as the package manager.

## Risks and notes

- **Astro `base` correctness.** Every internal link and asset path must resolve
  under `/right-agent`. Use Astro's link and asset helpers rather than hardcoded
  absolute paths, and verify the deployed site, not just the local dev server
  (the dev server can mask base-path mistakes).
- **Starlight and custom landing coexistence.** The custom `index.astro` owns the
  root route while Starlight owns the docs routes. Confirm the mounting during
  implementation (custom index plus docs under `/docs`).
- **Self-healing not applicable.** This is a static site with no agents or
  sandboxes; the platform's self-healing and upgrade rules do not apply here.

## Implementation cadence (for the plan)

The implementation plan must encode targeted intermediate checks and one final
full build, not a full build after every edit:

- Baseline: scaffold the Astro project, confirm `astro build` is green before
  adding content.
- Per slice (tokens and layout, landing sections, docs migration, README slim,
  deploy workflow): a narrow check appropriate to the slice.
- Final: one full `astro build` plus `astro check` plus link validation from a
  clean state, and a verified GitHub Pages deploy.
