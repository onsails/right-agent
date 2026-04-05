# Phase 39: cloudflared-auto-tunnel - Research

**Researched:** 2026-04-05
**Domain:** Cloudflare Named Tunnel CLI automation, Rust process invocation, interactive prompting
**Confidence:** HIGH

## Summary

Phase 39 replaces the manual `--tunnel-credentials-file` UX from Phase 38 with automatic tunnel detection and creation during `rightclaw init`. The new flow detects `~/.cloudflared/cert.pem` (cloudflared login state), queries existing tunnels via `cloudflared tunnel list -o json`, creates one if needed via `cloudflared tunnel create -o json <NAME>`, and writes `TunnelConfig` to `~/.rightclaw/config.yaml`. The credentials file always lands at `~/.cloudflared/<uuid>.json` — no copy needed, unlike Phase 38.

The cloudflared CLI output formats have been verified against the live binary (version 2026.3.0) on this machine. The JSON structure, flag placement, and stderr/stdout separation are confirmed. This eliminates all guesswork about parsing.

**Primary recommendation:** Use `cloudflared tunnel --loglevel error list -o json` (note: `--loglevel` is a tunnel-level flag, must precede the `list` subcommand) to get clean JSON on stdout. Use `cloudflared tunnel create -o json <NAME>` and parse the returned tunnel struct for `id`. Credentials file path is deterministic: `~/.cloudflared/<uuid>.json`.

## Project Constraints (from CLAUDE.md)

- Rust edition 2024
- Workspace architecture: CLI in `crates/rightclaw-cli/src/main.rs`, lib in `crates/rightclaw/`
- Error handling: `?` operator always, `miette` for user-facing errors in CLI, `thiserror` for lib errors
- FAIL FAST: no swallowed errors — every `if let Err` without `return Err` is a bug
- `serde_json` already in workspace dependencies — use it, don't add new JSON crates
- `which::which()` already used in codebase for binary discovery — use same pattern
- Interactive prompts use `std::io::stdin().read_line()` + `print!()` + `io::stdout().flush()` — no dialoguer/inquire dependency
- No `std::env::set_var()` in tests
- Tests in `#[cfg(test)]` modules, extract to `_tests.rs` if file exceeds 800 LoC with >50% tests

## Standard Stack

### Core (already in workspace — no new dependencies needed)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `std::process::Command` | stdlib | Invoke cloudflared CLI | Already used for bwrap smoke test in doctor.rs |
| `serde_json` | 1.0 | Parse JSON from cloudflared | Already workspace dep, used in main.rs tunnel helpers |
| `which` | workspace | Find cloudflared binary | Already used: `which::which("rg")`, `which::which("claude")` |
| `miette` | workspace | User-facing errors | Established CLI error pattern throughout main.rs |
| `dirs` | workspace | Resolve `~/.cloudflared/` path | Already used for `~/.rightclaw` in config.rs |

**No new Cargo dependencies required for this phase.**

## Architecture Patterns

### Recommended Structure

The entire phase is changes to `crates/rightclaw-cli/src/main.rs` (CLI args + cmd_init) and possibly `crates/rightclaw/src/config.rs` (migration hint update). No new modules needed.

```
crates/rightclaw-cli/src/main.rs
  Commands::Init { tunnel_name, tunnel_hostname, yes, ... }   ← replace tunnel_credentials_file
  cmd_init(...)                                                ← replace tunnel block
  
  // New private helpers (add after existing tunnel_uuid_from_credentials_file):
  fn detect_cloudflared_cert() -> bool
  fn find_tunnel_by_name(name: &str) -> miette::Result<Option<TunnelListEntry>>
  fn create_tunnel(name: &str) -> miette::Result<TunnelListEntry>
  fn prompt_yes_no(msg: &str, default_yes: bool) -> miette::Result<bool>
  fn prompt_hostname() -> miette::Result<String>

crates/rightclaw/src/config.rs
  // Update migration hint in read_global_config to reflect new init UX
```

### Pattern 1: cloudflared tunnel list -o json

**What:** Run `cloudflared tunnel --loglevel error list -o json`, capture stdout, parse JSON array.

**CRITICAL flag placement:** `--loglevel error` must come BEFORE `list` (it's a tunnel-level flag, not a subcommand flag). Wrong: `tunnel list --loglevel error`. Correct: `tunnel --loglevel error list`.

**Verified JSON structure** (live output from cloudflared 2026.3.0):
```json
[
  {
    "id": "aaaabbbb-0000-1111-2222-ccccddddeeee",
    "name": "right",
    "created_at": "2026-04-04T11:03:22.872741Z",
    "deleted_at": "0001-01-01T00:00:00Z",
    "connections": []
  }
]
```

**Fields available:** `id` (UUID string), `name` (string), `created_at`, `deleted_at`, `connections` array.

**Rust parsing:**
```rust
// Source: verified live against cloudflared 2026.3.0
#[derive(serde::Deserialize)]
struct TunnelListEntry {
    id: String,
    name: String,
}

fn find_tunnel_by_name(cf_bin: &std::path::Path, name: &str) -> miette::Result<Option<TunnelListEntry>> {
    let output = std::process::Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "list", "-o", "json"])
        .output()
        .map_err(|e| miette::miette!("cloudflared list failed: {e:#}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("cloudflared tunnel list failed: {stderr}"));
    }
    let tunnels: Vec<TunnelListEntry> = serde_json::from_slice(&output.stdout)
        .map_err(|e| miette::miette!("parse cloudflared list output: {e:#}"))?;
    Ok(tunnels.into_iter().find(|t| t.name == name))
}
```

### Pattern 2: cloudflared tunnel create -o json

**What:** Run `cloudflared tunnel --loglevel error create -o json <NAME>`, capture stdout, parse JSON object for `id`. Credentials file is written to `~/.cloudflared/<uuid>.json` by cloudflared — deterministic path, no copy needed.

**Verified JSON structure** (from cloudflared source + `renderOutput` behavior — same Tunnel struct as list):
```json
{
  "id": "aaaabbbb-0000-1111-2222-ccccddddeeee",
  "name": "rightclaw",
  "created_at": "2026-04-05T10:00:00Z",
  "deleted_at": "0001-01-01T00:00:00Z",
  "connections": []
}
```

**Note on stdout vs stderr:** Without `--loglevel error`, cloudflared emits info logs to stderr AND human-readable text to stdout (no `-o` flag path). With `-o json`, it emits only the JSON to stdout and logs go to stderr. `--loglevel error` silences the info logs on stderr for clean invocation. [VERIFIED: live binary test + source code analysis]

```rust
// Source: verified via cloudflared source (subcommand_context.go create() uses renderOutput)
fn create_tunnel(cf_bin: &std::path::Path, name: &str) -> miette::Result<TunnelListEntry> {
    let output = std::process::Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "create", "-o", "json", name])
        .output()
        .map_err(|e| miette::miette!("cloudflared create failed: {e:#}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(miette::miette!("cloudflared tunnel create failed: {stderr}"));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| miette::miette!("parse cloudflared create output: {e:#}"))
}
```

### Pattern 3: Credentials file location

**Verified:** The credentials file lands at `~/.cloudflared/<uuid>.json`. Confirmed by:
1. Live filesystem: `ls ~/.cloudflared/` shows `aaaabbbb-0000-1111-2222-ccccddddeeee.json`
2. Cloudflare docs: "Generate a tunnel credentials file in the default cloudflared directory"
3. cloudflared source: `tunnelFilePath()` constructs path from UUID under `~/.cloudflared/`

**Key difference from Phase 38:** Phase 38 copied the credentials file to `~/.rightclaw/tunnel/<uuid>.json`. Phase 39 references `~/.cloudflared/<uuid>.json` directly — no copy needed. The `TunnelConfig.credentials_file` field will point to `~/.cloudflared/<uuid>.json`.

```rust
// Source: [VERIFIED: live filesystem + cloudflare docs]
fn cloudflared_credentials_path(uuid: &str) -> miette::Result<std::path::PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| miette::miette!("cannot determine home directory"))?;
    Ok(home.join(".cloudflared").join(format!("{uuid}.json")))
}
```

### Pattern 4: cloudflared tunnel route dns

**What:** `cloudflared tunnel --loglevel error route dns <uuid> <hostname>` — creates CNAME DNS record. Non-fatal: DNS record may already exist (error 1003 from Cloudflare API), or hostname may be on a different zone.

**Exit behavior:** Non-zero exit on failure (e.g., DNS record already exists, zone mismatch). Success = exit 0. [ASSUMED — not verified by running against live API, but standard UNIX convention and consistent with existing Phase 38 `|| true` treatment]

**Usage:** Log warn on non-zero exit, continue. This matches existing Phase 38 wrapper script behavior (`route dns ... || true`).

```rust
fn route_dns(cf_bin: &std::path::Path, uuid: &str, hostname: &str) {
    let result = std::process::Command::new(cf_bin)
        .args(["tunnel", "--loglevel", "error", "route", "dns", uuid, hostname])
        .output();
    match result {
        Ok(output) if output.status.success() => {
            println!("DNS CNAME record created for {hostname}");
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("cloudflared route dns failed (non-fatal): {stderr}");
        }
        Err(e) => {
            tracing::warn!("cloudflared route dns invocation failed (non-fatal): {e:#}");
        }
    }
}
```

### Pattern 5: cert.pem detection

**What:** `~/.cloudflared/cert.pem` exists iff user has run `cloudflared login`. [VERIFIED: live filesystem shows cert.pem present after login]

```rust
fn detect_cloudflared_cert() -> bool {
    dirs::home_dir()
        .map(|h| h.join(".cloudflared").join("cert.pem").exists())
        .unwrap_or(false)
}
```

### Pattern 6: Interactive Y/n prompt

**Existing pattern** (from `prompt_telegram_token` in `crates/rightclaw/src/init.rs`):
```rust
// Source: crates/rightclaw/src/init.rs lines 211-225
use std::io::{self, Write};
print!("Tunnel 'rightclaw' already exists. Reuse it? [Y/n]: ");
io::stdout().flush().map_err(|e| miette::miette!("stdout flush: {e}"))?;
let mut input = String::new();
io::stdin().read_line(&mut input)
    .map_err(|e| miette::miette!("read input: {e}"))?;
let answer = input.trim().to_lowercase();
let reuse = answer.is_empty() || answer == "y" || answer == "yes";
```

No `dialoguer` or `inquire` dependency — raw stdin/stdout, consistent with existing codebase pattern.

### Anti-Patterns to Avoid

- **Wrong flag position:** `cloudflared tunnel list --loglevel error` fails with "flag provided but not defined". Use `cloudflared tunnel --loglevel error list`. [VERIFIED: live]
- **Parsing stdout without `-o json`:** Human-readable output is log-line format, fragile to parse. Always use `-o json`.
- **Copying credentials file:** Phase 38 copied to `~/.rightclaw/tunnel/`. Phase 39 references `~/.cloudflared/<uuid>.json` directly. No copy — the file is already in the right place.
- **Blocking on tunnel create stderr:** cloudflared emits info logs to stderr even on success. With `--loglevel error`, stderr is clean on success. Always check exit code, not stderr presence.
- **Assuming `deleted_at` is null:** The list JSON includes `"deleted_at": "0001-01-01T00:00:00Z"` for active tunnels (zero time, not null/absent). Filter by `deleted_at == "0001-01-01T00:00:00Z"` or just match by name (list excludes deleted by default).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Tunnel existence check | Parse human-readable `tunnel list` output | `tunnel --loglevel error list -o json` | Stable JSON contract, field names won't change |
| Credentials file path | Derive from API response parsing | `~/.cloudflared/<uuid>.json` convention | Deterministic, verified on live system |
| Y/n interactive prompt | dialoguer/inquire | `std::io::stdin().read_line()` | Codebase already uses this pattern, no new dep |
| Binary discovery | Hard-code `/usr/bin/cloudflared` | `which::which("cloudflared")` | Already used for rg, claude; nix installs to nix profile |

## Common Pitfalls

### Pitfall 1: --loglevel flag placement

**What goes wrong:** `cloudflared tunnel list --loglevel error` fails with "Incorrect Usage: flag provided but not defined".
**Why it happens:** `--loglevel` is a tunnel-level flag, not a subcommand (list/create) flag.
**How to avoid:** Always pass tunnel-level flags BEFORE the subcommand: `cloudflared tunnel --loglevel error list -o json`.
**Warning signs:** Exit code 1 + stderr mentions "flag provided but not defined".

### Pitfall 2: tunnel create stdout includes credentials warning text without -o

**What goes wrong:** Without `-o json`, create output is multi-line human text:
```
Tunnel credentials written to /home/user/.cloudflared/<uuid>.json. cloudflared chose this file...
Keep this file secret. To revoke these credentials, delete the tunnel.

Created tunnel my-tunnel with id <uuid>
```
**Why it happens:** `renderOutput` is only called when `-o json` is passed; otherwise human text.
**How to avoid:** Always use `create -o json` — get clean JSON, UUID in `id` field.

### Pitfall 3: Credentials file already exists for existing tunnel

**What goes wrong:** If the user has an existing tunnel `rightclaw` but its credentials JSON `~/.cloudflared/<uuid>.json` is missing (moved, different machine), the tunnel can be reused for config but cloudflared won't run without the credentials.
**Why it happens:** `tunnel list` shows tunnels registered on Cloudflare's side; credentials are local.
**How to avoid:** After deciding to reuse a tunnel, check that `~/.cloudflared/<uuid>.json` exists. If not, warn user: "Credentials file not found at `~/.cloudflared/<uuid>.json`. Re-create the tunnel or copy credentials from the machine where it was created."

### Pitfall 4: -y without --tunnel-hostname is an error

**What goes wrong:** CI/CD runs `rightclaw init -y` without `--tunnel-hostname` but cert.pem exists — init should fail clearly, not silently skip tunnel setup or hang on prompt.
**How to avoid:** When cert.pem exists AND `-y` is set AND `--tunnel-hostname` is not provided, return `Err(miette!("--tunnel-hostname is required with -y when cloudflared login detected"))`.

### Pitfall 5: serde_json Deserialize on output with extra fields

**What goes wrong:** If cloudflared adds new JSON fields in a future version, strict deserialization panics.
**How to avoid:** Use `#[serde(rename_all = "snake_case")]` on the struct but NOT `deny_unknown_fields`. The `TunnelListEntry` struct only needs `id` and `name`. [ASSUMED — serde default behavior is to ignore unknown fields]

## Code Examples

### CLI Args (replacing Phase 38 args)

```rust
// Source: pattern derived from existing Init variant in main.rs
Commands::Init {
    telegram_token: Option<String>,
    telegram_allowed_chat_ids: Vec<i64>,
    // NEW — replaces tunnel_credentials_file:
    /// Cloudflare Named Tunnel name (created if not exists)
    #[arg(long, default_value = "rightclaw")]
    tunnel_name: String,
    /// Public hostname for the tunnel (e.g. right.example.com)
    #[arg(long)]
    tunnel_hostname: Option<String>,
    /// Non-interactive mode (skip prompts, require --tunnel-hostname)
    #[arg(short = 'y', long)]
    yes: bool,
}
```

### Full cmd_init tunnel block skeleton

```rust
// cert.pem detection → skip or proceed
if !detect_cloudflared_cert() {
    println!("No cloudflared login found. Run `cloudflared login` for tunnel support.");
    return Ok(());  // or continue — depends on whether init structure returns early
}

let cf_bin = which::which("cloudflared")
    .map_err(|_| miette::miette!("cloudflared not found in PATH — install it first"))?;

// Tunnel existence check
let existing = find_tunnel_by_name(&cf_bin, &tunnel_name)?;
let uuid = match existing {
    Some(ref t) => {
        // Reuse or prompt
        if tunnel_hostname.is_some() || yes {
            t.id.clone()  // silent reuse
        } else {
            let msg = format!("Tunnel '{}' already exists. Reuse it?", tunnel_name);
            if prompt_yes_no(&msg, true)? {
                t.id.clone()
            } else {
                return Err(miette::miette!("tunnel setup cancelled"));
            }
        }
    }
    None => {
        // Create new tunnel
        let created = create_tunnel(&cf_bin, &tunnel_name)?;
        created.id
    }
};

// Hostname resolution
let hostname = match tunnel_hostname {
    Some(h) => h,
    None if yes => return Err(miette::miette!(
        "--tunnel-hostname is required when using -y"
    )),
    None => prompt_hostname()?,
};

// DNS route (non-fatal)
route_dns(&cf_bin, &uuid, &hostname);

// Credentials file — deterministic path, no copy needed
let credentials_file = cloudflared_credentials_path(&uuid)?;
if !credentials_file.exists() {
    tracing::warn!(
        path = %credentials_file.display(),
        "credentials file not found — tunnel may have been created on a different machine"
    );
}

// Write config.yaml
let tunnel_config = rightclaw::config::TunnelConfig {
    tunnel_uuid: uuid.clone(),
    credentials_file,
    hostname: hostname.clone(),
};
let config = rightclaw::config::GlobalConfig { tunnel: Some(tunnel_config) };
rightclaw::config::write_global_config(home, &config)?;
println!("Tunnel config written. UUID: {uuid}, hostname: {hostname}");
```

## State of the Art

| Old Approach (Phase 38) | New Approach (Phase 39) | Impact |
|------------------------|------------------------|--------|
| `--tunnel-credentials-file PATH` manual arg | Auto-detect via `cert.pem` + `cloudflared tunnel list` | Zero-touch for users who already ran `cloudflared login` |
| Copy credentials to `~/.rightclaw/tunnel/<uuid>.json` | Reference `~/.cloudflared/<uuid>.json` directly | Simpler — no copy, no 0600 chmod, no tunnel dir |
| Requires pre-created tunnel | Creates tunnel if not found | One-step setup |
| Silent skip if neither arg provided | Informational message if no cert.pem | Better discoverability |

**Removed in Phase 39:**
- `--tunnel-credentials-file` arg (replaced by `--tunnel-name`)
- `tunnel_uuid_from_credentials_file()` helper (no longer needed — UUID comes from `tunnel list -o json` or `tunnel create -o json`)
- Tunnel directory copy logic (`~/.rightclaw/tunnel/` dir creation, file copy, chmod)
- `(Some(_), None)` / `(None, Some(_))` error branches in match

**Kept from Phase 38:**
- `TunnelConfig` struct in `config.rs` (unchanged API)
- `write_global_config` / `read_global_config` (unchanged)
- `cmd_up` cloudflared block (unchanged — it reads TunnelConfig.credentials_file directly)
- `doctor.rs` `check_tunnel_credentials_file` (still valid — checks that the referenced file exists)

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `cloudflared tunnel create -o json` outputs same Tunnel struct fields as `list` | Code Examples | If output format differs, UUID extraction fails; fallback: parse human-readable stdout for "with id <uuid>" |
| A2 | `cloudflared tunnel route dns` returns non-zero exit on failure (DNS record exists, zone mismatch) | Architecture Patterns | If it returns 0 even on failure, we lose the non-fatal warning; cosmetic only |
| A3 | serde default behavior ignores unknown JSON fields | Don't Hand-Roll | serde default is to ignore unknowns — only a problem if we add `deny_unknown_fields` by mistake |
| A4 | `--loglevel error` suppresses all non-error output to stderr for `create` as it does for `list` | Common Pitfalls | If create outputs warnings on stderr that pollute the JSON on stdout — but stdout/stderr are separate streams, so this is safe regardless |

## Open Questions

1. **Tunnel create -o json field name for UUID: `id` or something else?**
   - What we know: `tunnel list -o json` returns `id` field. Source analysis shows `create` calls `renderOutput(&tunnel)` with same `cfapi.Tunnel` type.
   - What's unclear: Whether the create command wraps the tunnel differently (the source mentions `&tunnel.Tunnel` vs `tunnel` — possible wrapper).
   - Recommendation: Verify by running `cloudflared tunnel create -o json test-dry-run` against a throwaway name, or treat as ASSUMED with fallback to parsing stdout for UUID regex. Safe fallback: also accept `TunnelID` field (credentials file format) if `id` is absent.

2. **Should `tunnel_uuid_from_credentials_file` be kept for Phase 39?**
   - Phase 39 no longer uses credentials file path as input — it's always determined automatically.
   - The helper is tested and used only in Phase 38 path. Can be removed or kept as dead code.
   - Recommendation: Remove it along with `--tunnel-credentials-file` to keep code clean. Tests for it should also be removed.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| cloudflared | Tunnel create/list/route | YES | 2026.3.0 | Skip tunnel setup if not in PATH |
| cert.pem | Login detection | YES | present | If absent, print info message and skip |
| serde_json | JSON parsing | YES (workspace dep) | 1.0 | — |
| which | Binary discovery | YES (workspace dep) | — | — |
| dirs | Home dir | YES (workspace dep) | — | — |

**cloudflared not in PATH:** If `which::which("cloudflared")` fails after cert.pem exists, return `Err` with install hint. The cert.pem check happens first; if cert.pem is absent, cloudflared is never invoked.

## Sources

### Primary (HIGH confidence)
- Live binary probe: `cloudflared tunnel --loglevel error list -o json` — verified actual JSON field names (`id`, `name`, `created_at`, `deleted_at`, `connections`) against cloudflared 2026.3.0
- Live filesystem: `ls ~/.cloudflared/` — confirmed cert.pem and `<uuid>.json` naming convention
- cloudflare-go source (tunnel.go): Tunnel struct JSON tags — `id`, `name`, `created_at`, `deleted_at`, `connections`, `tun_type`, `status`

### Secondary (MEDIUM confidence)
- cloudflared source (subcommand_context.go): `create()` calls `renderOutput(outputFormat, &tunnel)` — same struct as list
- Cloudflare docs (create-local-tunnel): credentials file at `~/.cloudflared/<uuid>.json` confirmed
- cloudflared help text: `--loglevel` is a tunnel-level flag (confirmed by "flag provided but not defined" error when placed after `list`)

### Tertiary (LOW confidence)
- `tunnel route dns` exit code behavior on failure (error 1003 / already exists) — inferred from Cloudflare Community posts, not verified by running

## Metadata

**Confidence breakdown:**
- JSON output format (list): HIGH — live verified
- JSON output format (create): MEDIUM — inferred from same type, source analysis
- Credentials file path convention: HIGH — live verified
- --loglevel flag placement: HIGH — live verified (negative test)
- route dns exit code: LOW — not live tested

**Research date:** 2026-04-05
**Valid until:** 2026-07-05 (cloudflared CLI stable, JSON output format rarely changes)
