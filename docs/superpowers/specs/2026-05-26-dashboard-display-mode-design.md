# Dashboard Display Mode Design

## Goal

Make the Telegram Mini App dashboard open in normal mode by default, give the
user an explicit fullscreen control, remember the last chosen display mode
locally, and adapt navigation so normal mode does not spend the first viewport
on menu controls.

## Current Context

`crates/right-dashboard/frontend/src/telegram.ts` currently calls
`ready()`, `requestFullscreen()`, and `expand()` during initialization. This
forces fullscreen whenever the Telegram client supports it.

`crates/right-dashboard/frontend/src/App.vue` owns dashboard state and mounts
`AppShell.vue`. `AppShell.vue` renders the topbar and the top-level dashboard
tabs:

```text
Overview | Activity | Knowledge | Usage | Identity | Health
```

The existing CSS is responsive for content grids, but the top tab row still
wraps in narrow Telegram widths and can consume too much vertical space before
useful dashboard content appears.

Telegram Mini Apps expose `expand()`, `requestFullscreen()`,
`exitFullscreen()`, `isFullscreen`, `fullscreen_changed`, and
`fullscreen_failed`. They do not expose a "wide but not fullscreen" window
mode. Telegram launch modes are compact, normal, or fullscreen.

References checked 2026-05-26:

- https://core.telegram.org/bots/webapps
- https://core.telegram.org/api/web-events
- https://core.telegram.org/method/messages.requestMainWebView

## Selected Approach

Keep this as a frontend-only presentation preference.

Use local storage to persist a `normal` or `fullscreen` display mode. First
open defaults to `normal`. Remembering the last choice means restoring the
saved display mode on the next dashboard open: if the saved mode is
`fullscreen`, initialization requests fullscreen; otherwise it stays normal.

This avoids backend state for a per-device UI preference and keeps the feature
resilient across Telegram clients with partial fullscreen support.

## Product Behavior

On initial load:

1. Read `right-dashboard.display-mode` from `localStorage`.
2. Treat missing or invalid values as `normal`.
3. Call `Telegram.WebApp.ready()` when available.
4. Call `Telegram.WebApp.expand()` when available.
5. Call `Telegram.WebApp.requestFullscreen()` only when the saved mode is
   `fullscreen`.

The dashboard topbar includes a display-mode control:

- In normal mode, the control requests fullscreen and persists `fullscreen`.
- In fullscreen mode, the control requests normal mode and persists `normal`.
- If the Telegram client reports `fullscreen_changed`, the UI updates from the
  actual Telegram state.
- If event support is absent, the UI follows the requested mode as best effort.

Unsupported fullscreen does not block the dashboard. The saved display mode is
the user's preferred mode; the actual Telegram fullscreen state may differ
when a client rejects or cannot report fullscreen changes. A failed request
must leave the user on the current usable layout, not force fullscreen-only
navigation while the app is visibly still normal.

## Adaptive Navigation

Normal narrow mode uses bottom navigation instead of the current top wrapped
tab row.

Rules:

- Normal mode plus narrow viewport: render top-level tabs in a sticky/fixed
  bottom bar with horizontal scrolling.
- The bottom bar contains every enabled tab directly. It must not hide tabs
  behind a `More` item.
- Fullscreen mode or wider viewport: keep top navigation because the available
  screen space makes the menu acceptable.
- Content receives enough bottom padding in bottom-nav mode so the final rows
  are not hidden under the navigation bar.
- The fullscreen/normal control stays in the topbar, separate from tab
  navigation.

The visual direction approved during brainstorming is bottom navigation with
horizontally scrollable tab buttons.

## Technical Shape

Extend `TelegramWebApp` in `telegram.ts` with the optional fields and methods
used by Telegram Bot API 8+ fullscreen support:

```ts
// crates/right-dashboard/frontend/src/telegram.ts
isFullscreen?: boolean
exitFullscreen?: () => void
onEvent?: (eventType: string, handler: (...args: unknown[]) => void) => void
offEvent?: (eventType: string, handler: (...args: unknown[]) => void) => void
```

Add small frontend helpers in `telegram.ts` rather than scattering Telegram and
storage calls through Vue components:

- parse and normalize the stored display mode;
- persist mode changes defensively;
- initialize Telegram without unconditional fullscreen;
- request/exit fullscreen with optional API guards;
- subscribe to fullscreen events when available.

`App.vue` should own reactive display-mode state and pass it to `AppShell.vue`.
`AppShell.vue` should receive the current mode and emit mode-toggle events.
This preserves the existing shell boundary: `AppShell.vue` renders chrome,
while `App.vue` coordinates state and side effects.

## Storage

Use one local storage key:

```text
right-dashboard.display-mode
```

Allowed values:

```text
normal
fullscreen
```

Invalid values are ignored and replaced by normal-mode behavior. Storage
exceptions, including private browsing failures, must not break dashboard
startup.

## Error Handling

Telegram APIs remain optional. The dashboard must keep rendering when:

- `window.Telegram` is absent;
- `requestFullscreen()` throws;
- `exitFullscreen()` throws;
- `localStorage` read/write throws;
- fullscreen events are not supported;
- `fullscreen_failed` reports unsupported fullscreen.

Failures should not be silently converted into unrelated UI states. The display
mode should remain a best-effort presentation preference, not an availability
signal for API data.

When Telegram reports actual fullscreen state, layout should follow that
reported state. Without event support, layout may follow the requested display
mode as best effort, except when a synchronous fullscreen request/exit throws.

## Tests And Verification

Implementation should use TDD for behavior changes:

1. Update `telegram.test.ts` first so the old unconditional fullscreen behavior
   fails.
2. Add tests for default normal initialization, saved fullscreen
   initialization, invalid stored values, storage failures, and fullscreen
   request/exit guards.
3. Add focused shell/layout tests only if the current frontend test setup can
   mount Vue components without adding heavy harness code. Otherwise rely on
   typecheck/build plus manual browser inspection for layout.

Targeted verification after implementation:

```sh
devenv shell -- npm run test --prefix crates/right-dashboard/frontend
devenv shell -- npm run typecheck --prefix crates/right-dashboard/frontend
devenv shell -- npm run build --prefix crates/right-dashboard/frontend
```

Final verification before declaring code complete:

```sh
devenv shell -- cargo test --workspace
devenv shell -- cargo build --workspace
```

## Out Of Scope

- Server-side display preferences.
- Cross-device preference sync.
- New dashboard API routes or database tables.
- Browser `window.resizeTo()` or popup-window behavior.
- A Telegram launch-link mode change.
- A `More` tab bucket for normal mode.
