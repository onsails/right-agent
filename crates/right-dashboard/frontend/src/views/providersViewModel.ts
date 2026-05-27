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
