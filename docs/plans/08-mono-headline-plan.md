# Stage 08 — Monospace headline (herdr #3) — Plan

Executor implements in the stage worktree. Files: `site/src/styles/tokens.css`
and `site/src/styles/landing.css` ONLY. Spec: `08-mono-headline-spec.md`.
Small, surgical — no markup, no copy, no other selectors.

## Tasks

1. **`site/src/styles/tokens.css`** — add a headline token to `:root`, next to the
   other font vars (after `--display`):
   ```
   --headline: 'JetBrains Mono Variable', ui-monospace, monospace;
   ```
   (JetBrains Mono is already imported in `fonts.css`; do NOT add an import.)

2. **`site/src/styles/landing.css` — `h1` rule** (the `h1 { … }` rule with
   `font-size: clamp(2.4rem, 5.5vw, 4rem)`):
   - `font-family: var(--display)` → `font-family: var(--headline)`
   - `letter-spacing: -.02em` → `letter-spacing: -.04em`
   - leave everything else on the rule unchanged.

3. **`site/src/styles/landing.css` — `h2` rule** (the `h2 { … }` rule with
   `font-size: clamp(1.6rem, 3vw, 2.1rem)`):
   - `font-family: var(--display)` → `font-family: var(--headline)`
   - `letter-spacing:-.01em` → `letter-spacing:-.03em`
   - leave everything else unchanged.

4. **`site/src/styles/landing.css` — `h1 .glow` rule**:
   - remove the `transform: skewX(-18deg);` declaration.
   - keep `color: var(--ink); display: inline-block; position: relative;`.
   - do NOT touch `h1 .glow::after` (the underline).

Do NOT change `.wm`, `.card h3`, `.hname`, `.step .stitle`, or any other
`var(--display)` usage — those stay Chakra Petch.

## Verification (run in the stage worktree)

- `cd site && bun install && bun run check && bun run build` → all green.
- `rg -n '\--headline' site/src/styles/tokens.css` → present (1 hit).
- `rg -n 'var\(--headline\)' site/src/styles/landing.css` → exactly 2 hits (h1, h2).
- `rg -n 'skewX' site/src/styles/landing.css` → NO hits.
- `rg -n 'var\(--display\)' site/src/styles/landing.css` → still present for
  `.wm`, `.card h3`, `.hname`, `.step .stitle` (4 hits — h1/h2 no longer among them).

Leave changes uncommitted; the stage-runner commits and lands.
