import type { ProviderView } from '../types'

export function validateSlug(s: string): string | null {
  if (!s) return 'required'
  if (s.length > 32) return 'too long'
  if (!/^[a-z][a-z0-9-]*$/.test(s)) return 'must match [a-z][a-z0-9-]*'
  return null
}

export function validateEnvVar(s: string): string | null {
  if (!s) return 'required'
  if (s.length > 64) return 'too long'
  if (!/^[A-Z_][A-Z0-9_]*$/.test(s)) return 'must match [A-Z_][A-Z0-9_]*'
  return null
}

/** Known HTTP auth-scheme prefixes a user might wrongly paste into the key. */
const CREDENTIAL_SCHEME_PREFIXES = ['Bearer', 'Basic', 'Token', 'Bot', 'Digest'] as const

/**
 * Returns the canonical-cased auth-scheme prefix the value appears to start
 * with (e.g. "Bearer"), or null. Match is case-insensitive on the first
 * whitespace-delimited word and requires a following space + more text, so a
 * key that merely contains "bearer" as a substring is NOT flagged.
 */
export function detectCredentialPrefix(value: string): string | null {
  const trimmed = value.trimStart()
  const space = trimmed.indexOf(' ')
  if (space <= 0) return null
  const firstWord = trimmed.slice(0, space)
  const rest = trimmed.slice(space + 1).trim()
  if (rest.length === 0) return null
  return (
    CREDENTIAL_SCHEME_PREFIXES.find((p) => p.toLowerCase() === firstWord.toLowerCase()) ?? null
  )
}

/**
 * Decide whether a credential submit should proceed. The first time a
 * prefixed value is seen (`alreadyWarned === false`) it blocks and returns a
 * warning; once the user has been warned, the same value proceeds.
 */
export function evaluateCredentialSubmit(
  value: string,
  alreadyWarned: boolean,
): { proceed: boolean; warning: string | null } {
  const prefix = detectCredentialPrefix(value)
  if (prefix && !alreadyWarned) {
    return {
      proceed: false,
      warning: `This looks like it includes a "${prefix}" prefix. Providers store the bare key — the consumer adds it. Remove it, or press Save again to keep as-is.`,
    }
  }
  return { proceed: true, warning: null }
}

export function providerCompositionClass(provider: ProviderView): string {
  if (provider.composed === null) return 'warn'
  return provider.composed ? 'ok' : 'bad'
}

export function providerCompositionLabel(provider: ProviderView): string {
  if (provider.composed === null) return 'Unknown'
  return provider.composed ? 'Composed' : 'Not composed'
}

/** Microcopy shown under the Credential field (add/rotate). */
export const CREDENTIAL_HINT =
  'Paste only the key/token itself — no "Bearer ", no header name. The skill or agent adds any prefix.'

/** Microcopy shown under the Header name field (add/edit generic). */
export const HEADER_NAME_HINT =
  'HTTP header the skill/agent puts the key into for requests to the upstream host. Must match what the consumer sends (e.g. Authorization for Typefully).'
