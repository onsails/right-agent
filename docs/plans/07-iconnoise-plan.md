# Stage 07 — Reduce icon noise (drop caption icons) — Plan

Executor implements in the stage worktree. Files: `FlowStrip.astro`,
`Diagram.astro`, `index.astro`, `landing.css`. Spec: `07-iconnoise-spec.md`.
Markup/prop cleanup only — no copy changes, no node-icon changes.

## Tasks

1. **`site/src/components/FlowStrip.astro`.**
   - In `.how-cap`, remove the `<span class="ic-sm" set:html={svg(capIcon, 1.7)} />`
     so the line is just: `<div class="how-cap">{cap}</div>`.
   - Remove `capIcon` from the `Props` interface (`interface Props { cap: string;
     capIcon: string; nodes: Node[]; note?: string; }` → drop `capIcon: string;`).
   - Remove `capIcon` from the destructure (`const { cap, capIcon, nodes, note }`
     → `const { cap, nodes, note }`).

2. **`site/src/pages/index.astro`.** Remove the `capIcon="…"` attribute from both
   `<FlowStrip … />` usages (e.g. `capIcon="message"` and `capIcon="database"`).
   Leave `cap`, `note`, and `nodes` intact.

3. **`site/src/components/Diagram.astro`.** In BOTH `.how-cap` divs, remove the
   leading `<span class="ic-sm" set:html=…/>`:
   - "the round trip": `<div class="how-cap"><span class="ic-sm" …arrow…/>the round trip</div>`
     → `<div class="how-cap">the round trip</div>`.
   - "your credentials": `<div class="how-cap"><span class="ic-sm" …key…/>your credentials</div>`
     → `<div class="how-cap">your credentials</div>`.
   Leave the `.how-back` reply icon, the `.credkey` key icon, all node `.hicon`s,
   and the `icon` import (still used by `.harrow`/`.credkey`/`.how-back`).

4. **`site/src/styles/landing.css`.** Remove the now-unused `.how-cap .ic-sm { … }`
   rule. Keep the base `.ic-sm { … }` and `.ic-sm svg { … }` rules (still used by
   `.how-back`). Keep `.how-cap { … }` itself.

## Verification (run in the stage worktree)

- `cd site && bun install && bun run check && bun run build` → all green.
- `rg -n 'capIcon' site/src` → NO hits.
- `rg -n 'class="ic-sm"' site/src/components/Diagram.astro` → exactly 1 hit (the
  `.how-back` reply marker); none inside a `.how-cap`.
- `rg -n 'class="ic-sm"' site/src/components/FlowStrip.astro` → NO hits.
- `rg -n '\.how-cap \.ic-sm' site/src/styles/landing.css` → NO hits.

Leave changes uncommitted; the stage-runner commits and lands.
