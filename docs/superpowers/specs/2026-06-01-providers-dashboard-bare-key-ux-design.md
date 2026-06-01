# Providers Dashboard — "bare key" credential UX

**Date:** 2026-06-01
**Status:** Approved design (pre-implementation)
**Topic:** Make the provider dashboard explicit that a credential value is the bare key (no `Bearer `/scheme prefix), so users stop double-prefixing.

---

## Background / Problem

A generic provider (`right-typefully`) returned `401` on every Typefully
call while `right-twitterapi` worked. Root cause (empirically confirmed
from inside the sandbox against `GET https://api.typefully.com/v2/me`):

- The provider config is mechanically **correct** — `header_name:
  Authorization`, host `api.typefully.com`, endpoint ordering fine.
- The stored credential **value** was entered as `Bearer <token>`.
- The official Typefully skill (`/sandbox/.claude/skills/typefully/scripts/typefully.js:260`)
  builds the header itself as `Authorization: Bearer ${TYPEFULLY_API_KEY}`.
- Result: `Authorization: Bearer Bearer <token>` → `401`.
  - Proven: `Authorization: <env_var>` (bare) → **200** with real account
    data; `Authorization: Bearer <env_var>` → **401**.

The platform convention is: **the credential env var holds the bare
key; the consumer (skill/agent) adds any scheme prefix and chooses the
header.** The proxy resolves the placeholder inside whichever header the
consumer populates (the header whose name equals `config.header_name`),
substituting the stored value verbatim. `twitterapi` works because its
skill sends `x-api-key: ${key}` with a bare stored key.

The dashboard never expresses this convention. The `Header name`
field (example `Authorization`) sits next to `Credential (API key)`
with no explanation of their relationship, so users reasonably assume
the value should be `Bearer <key>`. **The UI is misleading.**

Note: the dashboard cannot show the *final* outgoing header — the
skill/agent assembles it in the sandbox and decides whether to prepend
`Bearer`. So a "live header preview" in the dashboard would itself be
inaccurate.

## Goal

Make the dashboard clearly communicate the "bare key" convention and
catch the specific double-prefix mistake before it reaches the gateway —
without guessing on the user's behalf.

## Decision

- **Convention A — "bare key only."** The `Credential` field stores only
  the key/token; framing (`Bearer `, etc.) is added by the consumer.
  (Rejected: B "full header value" conflicts with official skills that
  add `Bearer` themselves; C "framing toggle" adds complexity and still
  isn't synced with what the skill sends.)
- **Assist level 2 — microcopy + soft inline check.** Static hints plus a
  non-blocking warning when the entered value looks prefixed. (Rejected:
  level 3 hard-block — annoying on false positives, violates
  "control-plane doesn't guess for the user.")

## Scope

Frontend only. No backend / gateway / `agent.yaml` change — the existing
config is correct; this is purely a UX clarity fix.

- `crates/right-dashboard/frontend/src/views/providersViewModel.ts`
- `crates/right-dashboard/frontend/src/views/ProvidersView.vue`

Microcopy is **English** (the dashboard UI is English; no i18n is
introduced).

Applies at every credential-entry site:

| Site | Credential hint | Header-name hint | Prefix warning |
|------|:---------------:|:----------------:|:--------------:|
| Add — generic | ✓ | ✓ | ✓ (on Save) |
| Add — built-in | ✓ | n/a (no header_name field) | ✓ (on Save) |
| Rotate | ✓ | n/a | ✓ (on Save) |
| Edit — generic config | n/a (no credential field) | ✓ | n/a |

## Design

### 1. Pure detector (`providersViewModel.ts`)

```ts
const CREDENTIAL_SCHEME_PREFIXES = ['Bearer', 'Basic', 'Token', 'Bot', 'Digest'] as const

/**
 * Returns the auth-scheme prefix the value appears to start with (e.g.
 * "Bearer"), or null. Match is case-insensitive on the first
 * whitespace-delimited word and requires a following space + more text,
 * so a key that merely contains "bearer" as a substring is NOT flagged.
 */
export function detectCredentialPrefix(value: string): string | null {
  const trimmed = value.trimStart()
  const space = trimmed.indexOf(' ')
  if (space <= 0) return null
  const firstWord = trimmed.slice(0, space)
  const rest = trimmed.slice(space + 1).trim()
  if (rest.length === 0) return null
  const hit = CREDENTIAL_SCHEME_PREFIXES.find(
    (p) => p.toLowerCase() === firstWord.toLowerCase(),
  )
  return hit ?? null
}
```

Narrow known-scheme list keeps false positives near zero. Returns the
canonical-cased scheme name (from the constant) for the warning text.

### 2. Microcopy (`ProvidersView.vue`)

A new `.hint` element rendered under the relevant fields.

- **Credential** (Add-generic, Add-built-in, Rotate):
  > Paste only the key/token itself — no `Bearer `, no header name.
  > The skill or agent adds any prefix.
- **Header name** (Add-generic, Edit-generic):
  > HTTP header the skill/agent puts the key into for requests to the
  > upstream host. Must match what the consumer sends (e.g.
  > `Authorization` for Typefully).

### 3. Soft prefix warning (`ProvidersView.vue`)

State: `credentialWarnAck: Ref<boolean>` (one per flow that has a
credential field — add and rotate; reuse a single ref since only one
modal is open at a time).

`submitAdd` / `submitRotate` flow:

1. Run existing required/format validation first (unchanged).
2. Compute `prefix = detectCredentialPrefix(credentialValue)`.
3. If `prefix !== null` **and** `!credentialWarnAck`:
   - set a warning message, set `credentialWarnAck = true`, and **return
     without submitting**.
   - Warning text:
     > This looks like it includes a "{prefix}" prefix. Providers store
     > the bare key — the consumer adds it. Remove it, or press Save
     > again to keep as-is.
4. Otherwise submit as today.

Reset `credentialWarnAck = false` whenever the credential value changes
(so editing the value re-arms the check) and on modal open/close.

The warning reuses the existing `addError` / `rotateError` notice slot
(or a sibling `warn` ref styled less alarmingly — implementer's choice,
must be visually distinct from a hard error). It must not permanently
block: a second Save with the same value proceeds.

## Non-goals (YAGNI)

- Live preview of the final outgoing header (dashboard can't know if the
  skill prepends `Bearer`; preview would mislead).
- Backend validation of the credential prefix (value is a write-only
  secret; front-end entry-time check is sufficient and avoids handling
  the secret server-side).
- Framing toggle / scheme dropdown (rejected option C).
- i18n / Russian translation (UI is English).
- Any change to backend routes, gateway, `agent.yaml`, or the existing
  `header_name: Authorization` for typefully. Fixing the live typefully
  provider (re-enter the bare key via Rotate) is a separate user action,
  not part of this work.

## Testing

- **Unit** (`providersViewModel.ts`): `detectCredentialPrefix` —
  positives (`Bearer x`, `basic y`, `Token z`, `Bot t`, `Digest d`,
  leading spaces) and negatives (bare key, `bearertoken-no-space`, key
  containing "bearer" mid-string, empty, scheme-word with no trailing
  value).
- **Component (Vue SSR, `@vue/server-renderer` `renderToString`)**:
  - hints render under Credential (add/rotate) and Header name
    (add-generic/edit-generic);
  - a prefixed credential blocks the first Save and surfaces the warning;
  - a second Save with the same value proceeds (mock the API call);
  - changing the value re-arms the warning.

## Verification cadence

- Intermediate (TDD loop): targeted frontend unit + component tests for
  the two changed files only.
- Final (mandatory): full frontend test/build for the dashboard package
  and `devenv shell -- cargo test --workspace` from the worktree (no Rust
  changes expected, but the final workspace test is mandatory per
  AGENTS.md).
