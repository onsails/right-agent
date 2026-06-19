# Stage 07 — Reduce icon noise (drop caption icons) — Spec

## Problem (user-reported)

The flow blocks carry a small icon in their **caption** (`.how-cap`) AND an icon
on every node — too many icons, and some are literal duplicates:
- FlowStrip "every chat, its own session": caption icon = `message`, and the
  first node "your chats" icon = `message` → same icon twice.
- Diagram "the round trip": caption icon = `arrow`, but `arrow` is also the
  connector between nodes (`.harrow`) → duplicated/noisy.
- Diagram "your credentials": caption icon = `key`, and the `.credkey` callout
  row below also shows a `key` → duplicate.

## Fix

Make the captions **text-only**. Remove the leading icon (`.ic-sm` span) from
every `.how-cap` in both `FlowStrip.astro` and `Diagram.astro`. The caption then
reads as a clean mono uppercase label; the node icons (distinct, meaningful) and
the `.credkey`/`.how-back` markers stay. This removes all three duplicates and
cuts the icon count per block.

## In scope

- `FlowStrip.astro`: drop the `<span class="ic-sm" …capIcon…/>` from `.how-cap`
  (caption = `{cap}` text only). Remove the now-unused `capIcon` prop from the
  `Props` interface and the destructure.
- `index.astro`: remove the `capIcon="…"` attribute from both `<FlowStrip … />`
  usages (the prop no longer exists).
- `Diagram.astro`: drop the `<span class="ic-sm" …/>` from BOTH `.how-cap` divs
  ("the round trip" → was `arrow`; "your credentials" → was `key`). Captions
  become text-only.
- `landing.css`: remove the now-unused `.how-cap .ic-sm { … }` rule (the base
  `.ic-sm` rule stays — still used by `.how-back`).

## Out of scope — keep

- All node `.hicon` icons (telegram/cloudflare/box, claude/swap/globe,
  message/claude/database, shield/database/eye) — distinct and meaningful.
- The `.credkey` key icon (the "your keys stay on the host" callout) and the
  `.how-back` reply icon — meaningful, no longer duplicated once the caption icon
  is gone.
- Caption TEXT, node text, all copy — unchanged.
- Everything from stages 1–6.

## Acceptance criteria

1. No flow-block caption shows a leading icon; captions are text-only.
2. The duplicate icons are gone: "every chat" no longer repeats the message icon;
   "the round trip" caption no longer shows an arrow; "your credentials" caption
   no longer shows a key (the `.credkey` row still does, once).
3. Node icons, `.credkey`, and `.how-back` icons remain.
4. `capIcon` is fully removed (no dead prop, no leftover call-site attribute).
5. `cd site && bun run check` → 0 errors; `cd site && bun run build` → success.
6. Files: `FlowStrip.astro`, `Diagram.astro`, `index.astro`, `landing.css`.

## Verification (website-only — no cargo)

- `cd site && bun run check && bun run build` green.
- `rg -n 'capIcon' site/src` → no hits (prop + all call-site args gone).
- `rg -n 'how-cap.*ic-sm|ic-sm.*how-cap' site/src` and visually: no icon inside any `.how-cap`.
- `rg -n 'class="ic-sm"' site/src/components/Diagram.astro` → only the `.how-back` reply marker remains (1 hit, in the `<p class="how-back">`).
