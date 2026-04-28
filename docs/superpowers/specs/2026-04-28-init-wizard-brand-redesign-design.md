# Init wizard brand redesign — design

Status: design (approved)
Date: 2026-04-28
Scope owner: andrey

## Goal

Bring the init wizard and its sibling CLI surfaces into conformance with
the Right Agent brand guide (`docs/brand-guidelines.html`) — three terminal
atoms (`▐✓` mark, `▐` rail, wordmark), four semantic glyphs (`✓ ! ✗ …`)
with locked colors, and the lowercase-first / terse / factual voice. The
work also unifies presentation across five commands so the rest of the CLI
stops looking orphaned once the wizard is repainted.

## In scope

- `right init` — first-run global wizard (tunnel, telegram detection).
- `right agent init <name>` — per-agent wizard.
- `right config` and `right config set …` — settings menu and single-field setters.
- `right doctor` — diagnostics output.
- `right status` — process status output.

## Out of scope

- `right up`, `right down`, `right attach`, `right list`, `right restart` — copy unchanged.
- `wizard.rs` is 1556 LoC and mixes seven concerns. **Not refactored here.** Separate task.
- Process-compose TUI styling — process-compose owns it.
- Telegram bot user-facing strings.
- Brand assets in `assets/`.

## Decisions

| # | Decision |
|---|---|
| 1 | Rail (`▐`) prefix appears on splash + non-interactive output only. Interactive prompts (`inquire`) stay plain — no rail or glyph injected into the prompt itself. |
| 2 | Full splash (`▐✓ right agent vX.Y.Z` + tagline) appears only on `right init`. Other in-scope commands open with a one-line section header (`▐ name ─────`). |
| 3 | Init wizards (`right init`, `right agent init`) close with a rail-prefixed recap block + a `▐  next: …` pointer. `right config`, `doctor`, `status` do not have a recap. |
| 4 | Voice tiers — prompt labels and status lines are strict lowercase-first; help and error explanations are sentence-case but terse (1–2 short sentences). |
| 5 | Binary name in CLI text stays `right`. Splash/headers use the lowercase wordmark `right agent`. `Right Agent` (Title Case) appears only in long prose error sentences. |
| 6 | Canonical status-line shape: `▐  {glyph} {noun:<width}  {verb} [({detail})]` — two spaces between noun column and verb, optional parenthesised detail. |
| 7 | Three theme tiers — TTY+color → glyphs+color (`Theme::Color`); `NO_COLOR` set → glyphs+no-color (`Theme::Mono`); `TERM=dumb` or non-TTY → ASCII (`Theme::Ascii`: `\|` rail, `[ok]/[warn]/[err]/[…]` glyphs). |
| 8 | Shared module at `crates/right-agent/src/ui/` owns all atoms, glyphs, splash, headers, recaps, theme detection, writers. Doctor's `Display` impl migrates here. |
| 9 | `right status` keeps tabular density: rail-prefixed status lines with PID + uptime columns, one summary footer line. No platform/agent grouping. |
| 10 | `right doctor` becomes a rail+glyph block with indented `▐    fix: …` lines and a `{passed}/{total} checks passed (X warn, Y fail)` summary. |

## Architecture

### Module: `right-agent::ui`

Layout:

```
crates/right-agent/src/ui/
├── mod.rs       # public re-exports
├── atoms.rs     # Rail, Glyph
├── theme.rs     # Theme, detect, detect_with
├── line.rs      # Line, Block, status()
├── splash.rs    # splash()
├── header.rs    # section()
├── recap.rs     # Recap
├── writer.rs    # stdout(), stderr()
└── error.rs     # BlockAlreadyRendered sentinel
```

Public API (everything else stays private):

```rust
pub use atoms::{Glyph, Rail};
pub use theme::{Theme, detect};
pub use line::{Line, Block, status};
pub use splash::splash;
pub use header::section;
pub use recap::Recap;
pub use writer::{stdout, stderr};
pub use error::BlockAlreadyRendered;
```

#### `Theme` and detection

```rust
pub enum Theme { Color, Mono, Ascii }

pub fn detect() -> Theme;                              // cached in OnceLock
pub fn detect_with(tty: &impl IsTerminal, env: &impl EnvGet) -> Theme;
```

Detection order (first match wins):
1. `TERM=dumb` or `tty.is_terminal() == false` → `Ascii`.
2. `NO_COLOR` env var present (any non-empty value, per https://no-color.org) → `Mono`.
3. Otherwise → `Color`.

`EnvGet` is a tiny project-local trait with one `fn get(&self, key: &str) -> Option<String>` so tests can inject stubs without `std::env::set_var` (project rule).

#### Atoms

```rust
pub struct Rail;
impl Rail {
    pub fn prefix(theme: Theme) -> &'static str;   // "▐  " (Color/Mono) | "|  " (Ascii)
    pub fn mark(theme: Theme)   -> &'static str;   // "▐✓"  | "|*"
    pub fn blank(theme: Theme)  -> &'static str;   // "▐"   | "|"
}

pub enum Glyph { Ok, Warn, Err, Info }
impl Glyph {
    pub fn render(self, theme: Theme) -> String;
    // Color: colored Unicode glyph (✓ ! ✗ …)
    // Mono:  plain Unicode glyph
    // Ascii: bracketed text "[ok]", "[warn]", "[err]", "[…]"
}
```

Color tokens applied under `Theme::Color`:

| Atom | Hex | Use |
|---|---|---|
| Rail / mark | `#E8632A` | brand orange, only on `▐` and `▐✓` |
| Ok | `#6BBF59` | `✓` |
| Warn | `#D9A82A` | `!` |
| Err | `#E03C3C` | `✗` |
| Info | `#4A90E2` | `…` |
| Detail | `#888888` | parenthesised detail text in status lines |

Implemented via `owo-colors` truecolor escapes (`owo-colors` is already a dependency through `doctor.rs`).

#### Status-line builder

Canonical line shape: `▐  {glyph} {noun:<width}  {verb} [({detail})]`.

```rust
pub fn status(glyph: Glyph) -> Line;

pub struct Line { /* private */ }
impl Line {
    pub fn noun(self, s: impl Into<String>) -> Self;
    pub fn verb(self, s: impl Into<String>) -> Self;
    pub fn detail(self, s: impl Into<String>) -> Self;     // wrapped in (…)
    pub fn fix(self, s: impl Into<String>) -> Self;        // emits "▐    fix: …" second line
    pub fn render(self, theme: Theme) -> String;
}

pub struct Block { /* private */ }
impl Block {
    pub fn new() -> Self;
    pub fn push(&mut self, line: Line);
    pub fn render(self, theme: Theme) -> String;           // aligns noun column across pushed lines
}
```

`Block` computes the noun column width as `max(noun.len()) + 2`. A single `Line` rendered standalone uses no padding.

#### Splash, section header, recap

```rust
pub fn splash(theme: Theme, version: &str, tagline: &str) -> String;
// →
// ▐✓ right agent v0.10.2
// ▐  sandboxed multi-agent runtime
// ▐

pub fn section(theme: Theme, name: &str) -> String;
// → "▐ telegram ─────────────────────────"  (width 48; ─ becomes - in Ascii)

pub struct Recap { /* private */ }
impl Recap {
    pub fn new(title: &str) -> Self;                       // "ready", "saved"
    pub fn ok(self, noun: &str, detail: &str) -> Self;
    pub fn warn(self, noun: &str, detail: &str) -> Self;
    pub fn next(self, hint: &str) -> Self;                 // emits "▐  next: <hint>"
    pub fn render(self, theme: Theme) -> String;
}
```

#### Writers and error sentinel

```rust
pub fn stdout(theme: Theme, s: &str);
pub fn stderr(theme: Theme, s: &str);

pub struct BlockAlreadyRendered;       // returned when a command already printed
                                       // a rail block and just wants to exit nonzero
                                       // without miette double-printing.
```

`BlockAlreadyRendered` implements `std::error::Error` with empty `Display`. Calling code catches it and `std::process::exit(1)`s.

## Per-command flows

### `right init` (first-run)

`right init` is a full first-run wizard. Today's flow (preserved by this redesign):

1. Sandbox mode (openshell / none).
2. Network policy (restrictive / permissive — only when sandbox is openshell).
3. Telegram bot token (optional, with Esc-to-go-back).
4. Allowed chat IDs (only when token is set).
5. Memory provider config.
6. Tunnel setup (mandatory — writes `~/.right/config.yaml`).
7. Codegen for the default `right` agent + sandbox creation if openshell.

The redesign adds a splash, a dependency probe block before any prompts, section headers between prompt groups, status lines after each commit, and a final recap.

```
▐✓ right agent v0.10.2
▐  sandboxed multi-agent runtime
▐
▐ dependencies ─────────────────────────────
▐
▐  ✓ process-compose  v1.100.0
▐  ✓ openshell        ready
▐  ✓ claude           in PATH
▐  ! cloudflared      not in PATH (optional)
▐
▐ agent ────────────────────────────────────
▐
> sandbox mode (openshell / none) [openshell]: █
> network policy (restrictive / permissive) [restrictive]: █
▐
▐ telegram ─────────────────────────────────
▐
> telegram bot token (enter to skip): █
> your telegram user id (/start @userinfobot to find it, empty to skip): █
▐
▐ memory ───────────────────────────────────
▐
> memory provider (file / hindsight) [hindsight]: █
> hindsight api key (empty to rely on HINDSIGHT_API_KEY at runtime): █
▐
▐ tunnel ───────────────────────────────────
▐
> tunnel name (default: right): █
> tunnel hostname (e.g. right.example.com): █
▐
▐  … sandbox  creating
▐  ✓ sandbox  ready (right-right)
▐
▐ ready ────────────────────────────────────
▐
▐  ✓ agent       right (openshell, restrictive)
▐  ✓ telegram    @your_bot
▐  ✓ chat ids    1 allowed
▐  ✓ memory      hindsight
▐  ✓ tunnel      right.example.com
▐
▐  next: right up
```

Step order with brand additions:

1. **Splash** always shown.
2. **Dependency probe block** under `▐ dependencies ─────`. Fatal misses (`process-compose`, `claude`) emit `✗` lines and exit 1 silently via `BlockAlreadyRendered`. Non-fatal misses (`openshell`, `cloudflared`) emit `!` lines and continue.
3. **Agent section** under `▐ agent ─────` — existing sandbox-mode + network-policy prompts (rewritten per §"Voice rewrite"). Network policy step skipped when sandbox is `none`.
4. **Telegram section** under `▐ telegram ─────` — existing token + chat-ids prompts (Esc-back preserved).
5. **Memory section** under `▐ memory ─────` — existing `prompt_memory_config("right")` prompts.
6. **Tunnel section** under `▐ tunnel ─────` — existing `tunnel_setup` prompts. Commit emits `✓ tunnel  created/reused/recreated (host)`.
7. **Codegen + sandbox creation** — info line `… sandbox  creating` while the sandbox spawns, then `✓ sandbox  ready (name)` on completion.
8. **Recap** under `▐ ready ─────` listing every configured surface + `▐  next: right up`.

The current footer (`Initialized Right Agent at …` + `Default agent 'right' created at …` + `Setup complete. Next steps: …`) is replaced wholesale by the recap.

### `right agent init <name>`

```
▐ agent init: <name> ───────────────────────
▐
> sandbox mode (openshell / none) [openshell]: █
> network policy (restrictive / permissive) [restrictive]: █
> telegram bot token (required): █
> allowed chat ids (required, comma-separated): █
> enable voice transcription? [Y/n]: █
> whisper model (small / tiny / base / medium / large-v3) [small]: █
> memory provider (file / hindsight) [file]: █
▐
▐  ✓ agent  finance  created
▐
▐ ready ────────────────────────────────────
▐
▐  ✓ sandbox     openshell (restrictive)
▐  ✓ telegram    @your_finance_bot
▐  ✓ chat ids    1 allowed
▐  ✓ stt         small
▐  ✓ memory      file
▐
▐  next: right up
```

No splash — `▐✓` mark is reserved for `right init`. Prompt order is unchanged from current code (sandbox → policy → telegram → chat ids → stt → memory). Esc-back navigation (`inquire_back`) is preserved unchanged. Recap is only emitted on completion; Ctrl+C aborts skip the recap.

### `right config`

```
▐ config ───────────────────────────────────
▐
> settings:
  tunnel: right.example.com (a3f02e1c…)
  agent: finance
  agent: research
  done
```

After each submenu commit, one rail-prefixed status line:

```
▐  ✓ tunnel  saved
```

Then re-render the menu. No splash, no recap. `done` exits silently.

### `right config set …`

Same shape as a config submenu — section header `▐ config: <field> ─────`, prompts, single `▐  ✓ <field>  saved` line.

### `right doctor`

```
▐ diagnostics ──────────────────────────────
▐
▐  ✓ right            in PATH
▐  ✓ process-compose  v1.100.0
▐  ✓ claude           in PATH
▐  ! cloudflared      not in PATH (optional, needed for tunnel)
▐    fix: brew install cloudflared
▐  ✗ openshell        gateway unreachable
▐    fix: openshell gateway start
▐
▐  6/8 checks passed (1 warn, 1 fail)
```

`Display for DoctorCheck` is removed. `cmd_doctor` builds a `ui::Block`, pushes one `Line` per check, renders. Footer omits the `(X warn, Y fail)` clause when both counts are zero. Exit code unchanged: nonzero on any `✗`.

### `right status`

```
▐ status ───────────────────────────────────
▐
▐  ✓ right-mcp-server      12340  2h17m
▐  ✓ right-bot-finance     12345  2h17m
▐  ! right-bot-research    12380  restarting (3×)
▐
▐  3 processes (1 warn)
```

Glyph mapping: process-compose `Running` → `Ok`; `Restarting`, `Pending` → `Warn`; `Failed`, `Stopped`, `Skipped` → `Err`.

PC not running:

```
▐ status ───────────────────────────────────
▐
▐  ✗ right agent  not running
▐    fix: right up
```

## Voice rewrite

The full canonical-string table lives below. Patterns:

1. No Title Case prompt labels — first word lowercase always.
2. No exclamation marks anywhere user-facing.
3. No "Successfully X" — use past-tense verb only: `created`, `saved`, `deleted`, `reused`, `recreated`.
4. Status lines obey the canonical shape. No deviation.
5. Errors: lowercase first word, ≤ 1 short clause main text, recovery in `help =`.
6. Brand atoms emitted only via `ui::*`; direct hardcoding is a review-blocking defect.
7. `Right Agent` (Title Case) only in long prose error sentences, never in atoms / labels / status lines.
8. Env var names stay in canonical SHOUTY case.
9. `▐` is always followed by 2 spaces before content; section header is `▐ name ─────` with the dashes filling to col 48.
10. No emoji. Existing `⚠` characters all migrate to the `!` warn glyph rendered through `Glyph::Warn`.

### Tunnel (`wizard.rs`)

| Current | New |
|---|---|
| `Tunnel hostname (e.g. right.example.com):` | `tunnel hostname (e.g. right.example.com):` |
| `New tunnel name:` | `new tunnel name:` |
| `Tunnel name:` | `tunnel name:` |
| `What would you like to do?` | `existing tunnel — choose:` |
| `Reuse existing tunnel` | `reuse` |
| `Create a new tunnel with a different name` | `rename` |
| `Delete and recreate the tunnel` | `delete and recreate` |
| `Found tunnel '{n}' in your Cloudflare account (UUID: {uuid}...)` | rail line: `▐  ! tunnel  found "{n}" ({uuid}…)` |
| Multi-line credentials warning | rail line: `▐    note: credentials file missing on this machine. choose "delete and recreate" to regenerate.` |
| `This will permanently delete tunnel '{n}'. Continue?` | `delete tunnel "{n}" permanently?` |
| `tunnel deletion cancelled` | `cancelled` |
| `Created tunnel '{n}' (UUID: {uuid})` | `▐  ✓ tunnel  created ({n})` |
| `Recreated tunnel '{n}' (UUID: {uuid})` | `▐  ✓ tunnel  recreated ({n})` |
| `Deleted tunnel '{n}'` | `▐  ✓ tunnel  deleted ({n})` |
| `Tunnel hostname must be a bare domain, not a URL` (help: `Use just the domain, e.g. right.example.com`) | `hostname must be a bare domain, not a url` / help `use just the domain, e.g. right.example.com` |
| `Tunnel credentials file not found at {p} — cloudflared cannot start without it` | `tunnel credentials missing at {p} — cloudflared cannot start` |

### Telegram (`wizard.rs`)

| Current | New |
|---|---|
| `Telegram bot token (current: ****..., press Enter to keep):` | `telegram bot token (keeping {masked} — enter new or press enter to keep):` |
| `Telegram bot token (required — get one from @BotFather):` | unchanged |
| `Telegram bot token (press Enter to skip):` | `telegram bot token (enter to skip):` |
| `A Telegram bot token is required. Talk to @BotFather to create a bot and get its token, then paste it here. Press Esc to go back.` | `a token is required. create a bot via @BotFather, paste the token here. esc to go back.` |

### Chat IDs (`wizard.rs`)

| Current | New |
|---|---|
| `Your Telegram user ID (required — send /start to @userinfobot to find it):` | `your telegram user id (required — /start @userinfobot to find it):` |
| `Your Telegram user ID (send /start to @userinfobot to find it, empty to skip):` | `your telegram user id (/start @userinfobot to find it, empty to skip):` |
| `At least one Telegram chat/user ID is required so the bot knows who is allowed to talk to it. Send /start to @userinfobot to find your numeric ID, then paste it here. Press Esc to go back.` | `at least one chat id is required so the bot knows who can talk to it. /start @userinfobot for your numeric id. esc to go back.` |
| `invalid chat ID '{}': {e}` | `invalid chat id "{}": {e}` |

### Sandbox / network / model / memory / STT (`wizard.rs`)

| Current | New |
|---|---|
| `OpenShell — run in isolated container (recommended)` | `openshell — isolated container (recommended)` |
| `None — run directly on host (for computer-use, Chrome, etc.)` | `none — direct host access (computer-use, chrome)` |
| `Sandbox mode:` | `sandbox mode:` |
| `Restrictive — Anthropic/Claude domains only (recommended)` | `restrictive — anthropic/claude domains only (recommended)` |
| `Permissive — all HTTPS domains allowed (needed for external MCP servers)` | `permissive — all https domains (needed for external mcp servers)` |
| `Network policy for sandbox:` | `network policy:` |
| `Allowed chat IDs (comma-separated, empty to clear):` | `allowed chat ids (comma-separated, empty to clear):` |
| `Enable voice transcription?` | `enable voice transcription?` |
| `Telegram voice messages and video notes will be transcribed locally via whisper.cpp.` | `telegram voice + video notes are transcribed locally via whisper.cpp.` |
| `Choose whisper model:` | `whisper model:` |
| `Hindsight API key source:` | `hindsight api key source:` |
| `Use HINDSIGHT_API_KEY env var (recommended)` | unchanged (env var name preserved) |
| `Enter a key to save in agent.yaml` | `enter a key to save in agent.yaml` |
| `Hindsight API key:` | `hindsight api key:` |
| `Hindsight API key (empty to rely on HINDSIGHT_API_KEY env var at runtime):` | `hindsight api key (empty to rely on HINDSIGHT_API_KEY at runtime):` |
| `Switching memory provider will not migrate existing memory. Continue?` | `switching memory provider does not migrate existing memory. continue?` |
| `Validating key against Hindsight...` | rail line: `▐  … hindsight  validating key` |
| `✓ Key valid — {banks} bank(s) accessible.` | `▐  ✓ hindsight  {banks} bank(s) accessible` |
| `Hindsight rejected the key (HTTP {status}). Save anyway?` | `hindsight rejected the key (http {status}). save anyway?` |
| `⚠ Could not validate (Hindsight unreachable): {detail}` | `▐  ! hindsight  unreachable ({detail})` |
| `Save config anyway?` | `save anyway?` |
| `⚠ No key available to validate (none entered, HINDSIGHT_API_KEY unset). Saving without validation.` | `▐  ! hindsight  no key — saving without validation` |
| `ffmpeg required for voice transcription. Install via 'brew install ffmpeg'?` | `ffmpeg required for voice transcription. install via brew?` |
| `STT will be disabled. Install ffmpeg later: brew install ffmpeg` | `▐  ! stt  disabled (install ffmpeg: brew install ffmpeg)` |
| `brew install ffmpeg exited with {status}; STT disabled.` | `▐  ✗ ffmpeg  install failed ({status})` + `▐  ! stt  disabled` |
| `brew completed but ffmpeg not yet in PATH — restart shell or check PATH; STT disabled.` | `▐  ! ffmpeg  not in PATH yet — restart shell` + `▐  ! stt  disabled` |
| Linux ffmpeg help block | rewritten as plain lowercase, two short lines: `ffmpeg required for voice transcription. install:` + indented packages list |
| `Saved.` | replaced everywhere with appropriate `▐  ✓ <noun>  saved` |

### Settings menus (`wizard.rs`)

| Current | New |
|---|---|
| `Settings:` | `settings:` |
| `Done` | `done` |
| `Agent: {name}` (option label) | `agent: {name}` |
| `Tunnel: {host} ({uuid})` (option label) | `tunnel: {host} ({uuid})` |
| `Agent '{name}' settings:` | section header: `▐ agent: {name} ─────` |
| `Telegram token: ****` etc. (option labels) | unchanged shape, lowercased: `telegram token: ****`, `model: sonnet`, etc. |
| `Select agent:` | `select agent:` |
| `No agents found in {dir}` | `no agents found in {dir}` |
| `Global config saved.` | `▐  ✓ tunnel  saved` |

### Doctor / status / misc (`main.rs`)

| Current | New |
|---|---|
| Doctor footer `\n  {p}/{t} checks passed` | `▐\n▐  {p}/{t} checks passed (X warn, Y fail)` (warn/fail clauses only when nonzero) |
| `Some checks failed. See above for fix instructions.` | `checks failed — see above for fixes` |
| Status table header `NAME STATUS PID UPTIME` | dropped — replaced by the rail+glyph block |
| Status `No processes running.` | `▐  ✗ right agent  not running` + fix line |
| `No running instance found. Is right running?` (help: `Start right first with right up`) | `not running` / help `right up` |
| `right is already running. Use \`right down\` first or \`right attach\` to connect.` | `already running — use "right down" or "right attach"` |

### `right init` footer (`cmd_init` in `main.rs`)

The current six-line footer is replaced wholesale by the rail-prefixed recap block defined in §"Per-command flows · `right init`". The individual lines below are listed only so the implementer knows what to delete:

| Current | Replacement |
|---|---|
| `Initialized Right Agent at {p}` | dropped — recap block is the new acknowledgement |
| `Default agent 'right' created at {p}/agents/right/` | dropped |
| `Telegram channel configured.` | recap line `▐  ✓ telegram    @{handle}` (or `! telegram    not configured`) |
| `Telegram chat ID allowlist configured.` | recap line `▐  ✓ chat ids    {n} allowed` |
| `Network policy: {p}` | recap line `▐  ✓ agent       right ({sandbox-mode}, {policy})` |
| `Setup complete. Next steps:` + 3-line list | `▐  next: right up` |
| `Creating OpenShell sandbox...` (info before sandbox spawn) | rail line `▐  … sandbox  creating` |
| `  Sandbox '{n}' ready` | rail line `▐  ✓ sandbox  ready ({n})` |
| `Setup cancelled.` (Esc on first step error) | `cancelled` |

## Error handling

### Error shape

```rust
miette::miette!(
    help = "right up",
    "not running"
)
```

Rules:

- Main text: lowercase first letter, no trailing period, ≤ 1 short clause.
- `help =` field present whenever a recovery action exists. Command-form when one command can fix it, otherwise a short imperative.
- No multi-paragraph error bodies. Long context goes to `tracing::error!` (logs) or a rail-prefixed status line printed before the error returns.
- No emoji, no apologies.

### Validation re-prompt loops

Today's loops `eprintln!` the error and re-loop. New behaviour: invalid input prints **one rail-prefixed warn line** above the re-prompt, then the prompt re-renders.

```rust
let trimmed = input.trim();
if let Err(e) = validate_telegram_token(trimmed) {
    ui::stderr(theme, &ui::status(Glyph::Warn)
        .noun("invalid")
        .verb(format!("{e:#}"))
        .render(theme));
    continue;
}
```

```
> telegram bot token (required): badtoken
▐  ! invalid  bot token must be in the form 123:abc
> telegram bot token (required): █
```

### Fatal errors during init

When `right init`'s dependency probe finds a fatal miss, the wizard exits with the `✗`-glyph rail line already rendered + no stack trace:

```
▐  ✗ claude  not in PATH
▐    fix: https://docs.anthropic.com/en/docs/claude-code
```

Implementation: probe collects all dependency results, renders the block, then on any fatal returns `Err(BlockAlreadyRendered.into())`. `cmd_init` catches it and `std::process::exit(1)`s without re-printing.

### Ctrl+C cancellation

`inquire_back` already converts Ctrl+C into a confirm prompt. Copy:

| Current | New |
|---|---|
| `Cancel setup?` | `cancel?` |

On confirmed cancel, emit one line and exit code 130 (`128 + SIGINT`):

```
▐  ! cancelled
```

### Non-interactive mode

Today's `right init --non-interactive` errors when a value is missing (`tunnel hostname is required in non-interactive mode (use --tunnel-hostname)`). New: same error, lowercase, with `help =` pointing at the missing flag. No prompt, no rail line — non-interactive is single-shot diagnostic mode and `miette` formats it correctly already.

### `tracing` vs user output

`tracing::warn!` / `tracing::error!` call sites unchanged — they go to log files, not the user's terminal in interactive mode. We do not double-emit. Anywhere code currently does both `tracing::warn!(...)` AND `eprintln!(...)`, the tracing call stays and the eprintln becomes a `ui::status(...)` rail line.

## Testing

### Unit tests in `right-agent::ui`

Tests live next to the module (`atoms_tests.rs`, `line_tests.rs`, `splash_tests.rs`, `recap_tests.rs`, `theme_tests.rs`) per the project's >800 LoC + >50% test rule.

Coverage matrix — every renderer × every theme:

```rust
#[test] fn rail_prefix_color()  { /* "▐  " with orange ANSI */ }
#[test] fn rail_prefix_mono()   { /* "▐  " no ANSI */ }
#[test] fn rail_prefix_ascii()  { /* "|  " */ }
// rail_mark_*, rail_blank_*, glyph_ok_*, glyph_warn_*, glyph_err_*, glyph_info_*
```

Snapshot tests via `insta` (already a workspace dep; if not, add):

```rust
#[test] fn splash_color_matches_brand_example() {
    let s = ui::splash(Theme::Color, "0.10.2", "sandboxed multi-agent runtime");
    insta::assert_snapshot!(s);
}
#[test] fn splash_ascii_no_ansi()         { /* assert no \x1b in output */ }
#[test] fn splash_mono_unicode_no_ansi()  { /* assert ▐ present, no \x1b */ }
```

Block alignment:

```rust
#[test] fn block_aligns_noun_column() {
    let mut b = ui::Block::new();
    b.push(ui::status(Glyph::Ok).noun("right").verb("in PATH"));
    b.push(ui::status(Glyph::Warn).noun("cloudflared").verb("not in PATH"));
    let s = b.render(Theme::Mono);
    assert_eq!(extract_verb_col(&s, 0), extract_verb_col(&s, 1));
}
```

Theme detection uses injected stubs (no `set_var`):

```rust
#[test] fn detect_dumb_term_returns_ascii() { /* TERM=dumb → Ascii */ }
#[test] fn detect_no_color_returns_mono()   { /* NO_COLOR=1 → Mono */ }
#[test] fn detect_non_tty_returns_ascii()   { /* IsTerminal=false → Ascii */ }
#[test] fn detect_tty_no_env_returns_color(){ /* defaults → Color */ }
```

### Integration tests via `assert_cmd`

All run with `NO_COLOR=1` (passed via `Command::env()`, never `set_var`) for deterministic snapshots.

| Test | Command | Asserts |
|---|---|---|
| `init_first_run_recap` | `right --home <tmp> init --tunnel-name right --tunnel-hostname right.test` (mock cloudflared in PATH) | stdout contains `▐✓ right agent v` and `▐  ✓ tunnel    right.test` and `▐  next: right agent init` |
| `init_rerun_writes_recap` | second `right init` against same home (with same flags) | stdout contains `▐ ready ──` and recap `▐  ✓ agent       right` (existing rewrite-on-rerun behavior preserved) |
| `agent_init_recap` | `right agent init finance --non-interactive --telegram-token … --chat-ids …` | stdout contains `▐ agent init: finance ──` and `▐  ✓ agent  finance  created` and recap block |
| `doctor_renders_block` | `right --home <tmp> doctor` | stdout contains `▐ diagnostics ──` and `checks passed` shape |
| `status_no_pc_running` | `right --home <tmp> status` | stdout contains `▐  ✗ right agent  not running` and `▐    fix: right up` |
| `ascii_fallback_no_unicode` | `TERM=dumb right --home <tmp> doctor` | stdout matches `^[|]` per line, no `▐ ✓ ✗ ! …` codepoints |
| `mono_no_ansi_escapes` | `NO_COLOR=1 right --home <tmp> doctor` | stdout contains `▐` but no `\x1b` |

Mock cloudflared lives at `tests/fixtures/cloudflared-mock.sh` (shell script handling `tunnel list -o json` and `tunnel create -o json` returning canned JSON) — pattern already used in current tests.

### Voice-pass regression test

A single test enumerates every prompt label string by extracting them from a centralised `pub(crate) const PROMPT_LABELS: &[&str]` colocated with prompt definitions, and asserts:

```rust
#[test] fn no_title_case_prompts() {
    for label in PROMPT_LABELS {
        let first = label.chars().next().unwrap();
        assert!(
            !first.is_uppercase() || ALLOWED_PROPER_NOUNS.iter().any(|p| label.starts_with(p)),
            "prompt starts with uppercase: {label:?}"
        );
    }
}
#[test] fn no_exclamation_marks() {
    for label in PROMPT_LABELS {
        assert!(!label.contains('!'), "prompt contains '!': {label:?}");
    }
}
```

`ALLOWED_PROPER_NOUNS` includes env var names and `@`-handles — `HINDSIGHT_API_KEY`, `RIGHT_TG_TOKEN`, `@BotFather`, `@userinfobot`. Forces future contributors to register new prompt strings in one place.

### Brand-conformance lint

A `#[test]` that walks an exported snapshot of every `Block` rendered by every command path under `Theme::Mono` and asserts:

- Every line either starts with `▐` (or is empty) or is an `inquire` prompt placeholder string explicitly marked as "raw".
- No status line contains `Successfully` / `Successful` / `successfully`.
- No status line ends with `.`.

Implementation: each command's user-facing rail output goes through a `ui::Sink` trait (also used by snapshot tests), so capturing for the lint costs nothing extra at runtime.

### No `#[ignore]`

Per `CLAUDE.rust.md` and project memory, no integration test gets `#[ignore]`. The cloudflared mock and OpenShell test infrastructure ensure every test runs on every dev machine.

## Implementation sequencing

Six steps, each compiles, each ships value alone, each has a verification gate. Order is chosen so a partial revert at any step leaves the codebase strictly better than before.

### Step 1 — `right-agent::ui` skeleton

Build:
- `crates/right-agent/src/ui/{mod,atoms,theme,line,splash,header,recap,writer,error}.rs`.
- Public API stubs from §"Architecture".
- Theme detection wired with real env/tty reads + injected-stub trait.
- Reuse `owo-colors` (already in tree).
- Add `insta` if not already a workspace dev-dep.

Verify:
- `cargo test -p right-agent ui::` — every atom × theme test passes.
- `cargo build --workspace` (debug).

Touches no command code. Module is self-contained; if the rest of the project ships without later steps, this module is dead but harmless.

### Step 2 — migrate `cmd_doctor`

Build:
- Remove `Display for DoctorCheck`.
- `cmd_doctor` builds a `ui::Block`, pushes one `Line` per check, renders the block + summary footer.
- Section header `▐ diagnostics ─────`.
- Error-text rewrites per §"Voice rewrite".

Verify:
- Existing doctor unit tests pass unchanged (the data model is untouched).
- New `assert_cmd` `doctor_renders_block` test passes.
- `NO_COLOR=1 right doctor` snapshot matches.
- `TERM=dumb right doctor` produces ASCII output.

Doctor first because it's the smallest non-interactive surface and exercises the full atom set against real check data before the wizard touches it.

### Step 3 — migrate `cmd_status`

Build:
- Replace `printf` table in `cmd_status` with a `ui::Block`.
- Glyph mapping (Running→Ok, Restarting/Pending→Warn, Failed/Stopped/Skipped→Err).
- "not running" branch emits the err line + fix.

Verify:
- New `assert_cmd` tests (status running, status not running, status with mixed states) pass.
- Manual smoke: `right up` in a separate worktree, then `right status` from this branch.

### Step 4 — `right init` redesign

Build:
- Splash + dependency probe block (with `BlockAlreadyRendered` sentinel for fatal-miss exit).
- Section headers `▐ dependencies ─────`, `▐ agent ─────`, `▐ telegram ─────`, `▐ memory ─────`, `▐ tunnel ─────`, `▐ ready ─────`.
- All existing `inquire` prompts (sandbox, network, telegram, chat-ids, memory, tunnel) preserved; copy rewritten per §"Voice rewrite".
- `… sandbox  creating` info line + `✓ sandbox  ready` line around the sandbox spawn.
- Recap block replaces the current six-line footer (every line listed in §"Voice rewrite · `right init` footer").
- `BlockAlreadyRendered` sentinel for "block printed, exit silently".

Verify:
- New `assert_cmd` tests `init_first_run_recap`, `init_rerun_writes_recap` pass.
- Existing `right init` integration tests pass (with copy-string assertions updated mechanically).
- Manual: `right init --home /tmp/right-fresh` visually matches the brand splash example.

### Step 5 — `right agent init` + `right config` redesign

Build:
- Section headers for `agent init: <name>`, `agent: <name>`, `config`.
- Copy rewrites from §"Voice rewrite" §"Sandbox / network / model / memory / STT" and §"Settings menus".
- Recap block after `right agent init`.
- Per-commit status lines in the config menu.
- Validation re-prompt loop emits warn lines.

Verify:
- `agent_init_recap` integration test passes.
- Voice-pass regression tests (`no_title_case_prompts`, `no_exclamation_marks`) pass against `PROMPT_LABELS`.
- Existing wizard memory/STT tests pass (they cover YAML mutation, not copy).
- Manual smoke pass through every option in `right config` against a real home.

### Step 6 — fallback hardening + brand-conformance lint

Build:
- Brand-conformance test (§"Brand-conformance lint") — capture every command's rendered output and assert.
- `ascii_fallback_no_unicode` and `mono_no_ansi_escapes` integration tests.
- `right --no-color` global flag forces `Theme::Mono` regardless of env.

Verify:
- `cargo test --workspace` clean.
- `cargo clippy --workspace -- -D warnings` clean.
- Final `cargo build --workspace` (debug).
- Manual:
  - `right doctor | cat` (non-TTY → ASCII).
  - `NO_COLOR=1 right init` (mono, glyphs, no ANSI).
  - `TERM=dumb right status` (ASCII rail, `[ok]/[warn]/[err]`).

### Order rationale

- Steps 1–3 are non-interactive; they prove the renderer without entangling `inquire`.
- Step 4 is the showcase — `right init` first-run is what users actually see first.
- Step 5 is the bulk of copy work but compiles + tests against the patterns locked by 1–4.
- Step 6 is hardening; if 1–5 ship and 6 doesn't, every in-scope command is brand-conforming and only fallback robustness is left.
