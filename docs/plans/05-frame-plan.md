# Stage 05 — herdr-style hairline frame — Plan

Executor implements in the stage worktree. Files: `site/src/layouts/Landing.astro`
(one markup wrapper) and `site/src/styles/landing.css`. Spec: `05-frame-spec.md`.
All rules are `1px solid var(--line)`. No textures/glows/colors, no copy changes.

## Tasks

1. **`Landing.astro` — wrap the nav for a full-bleed rule.** Currently:
   ```
   <div class="wrap">
     <nav class="topnav"> … </nav>
   </div>
   ```
   Wrap that `.wrap` in a full-width `<header class="topbar">`:
   ```
   <header class="topbar">
     <div class="wrap">
       <nav class="topnav"> … </nav>
     </div>
   </header>
   ```
   (Only add the wrapper; leave the nav markup inside unchanged.)

2. **`landing.css` — nav rule.** Add: `.topbar { border-bottom: 1px solid var(--line); }`

3. **`landing.css` — hero frame.**
   - `.hero` rule: add `border-bottom: 1px solid var(--line);`
   - `.hero-grid` rule: change `gap: 3rem;` → `gap: 0;` and `align-items: center;`
     → `align-items: stretch;`
   - `.hero-copy`: add `border-right: 1px solid var(--line); padding-right: 3rem;
     display: flex; flex-direction: column; justify-content: center;`
     (if `.hero-copy` has no rule yet, add one).
   - `.shot` rule: add `padding-left: 3rem;`
   - In the existing `@media (max-width: 900px)` block that sets
     `.hero-grid { grid-template-columns: 1fr; gap: 2rem; }`, also add:
     `.hero-copy { border-right: none; padding-right: 0; }` and
     `.shot { padding-left: 0; }`

4. **`landing.css` — section rules + footer.**
   - `.section` rule: add `border-bottom: 1px solid var(--line);`
   - `footer.sitefooter` rule: remove its `border-top: 1px solid var(--line);`
     declaration (the last section's border-bottom now draws that line).

5. **`landing.css` — integrate screenshot.** `.shot img` rule: remove
   `border-radius: 14px;` and `border: 1px solid var(--line);` (keep its sizing
   declarations: display, max-width, width, height, max-height).

## Verification (run in the stage worktree)

- `cd site && bun install && bun run check && bun run build` → all green.
- Sanity greps:
  - `rg -n '\.topbar' site/src/styles/landing.css` → present.
  - `rg -n 'border-right: 1px solid var\(--line\)' site/src/styles/landing.css` → present (hero-copy).
  - `rg -n 'border-top:1px solid var\(--line\)|border-top: 1px solid var\(--line\)' site/src/styles/landing.css` → footer's is gone (no hit, or only unrelated).

Leave changes uncommitted; the stage-runner commits and lands.
