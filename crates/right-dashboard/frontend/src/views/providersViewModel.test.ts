import { describe, expect, it } from 'vitest'

import {
  detectCredentialPrefix,
  evaluateCredentialSubmit,
  providerCompositionClass,
  providerCompositionLabel,
  CREDENTIAL_HINT,
  HEADER_NAME_HINT,
} from './providersViewModel'
import type { ProviderView } from '../types'

function providerView(overrides: Partial<ProviderView> = {}): ProviderView {
  return {
    name: 'hostagent-acme',
    type: 'generic',
    label: 'acme',
    env_var: 'ACME_API_KEY',
    generic: null,
    updated_at: null,
    composed: false,
    status: { kind: 'healthy' },
    ...overrides,
  }
}

describe('detectCredentialPrefix', () => {
  it('flags known auth-scheme prefixes (case-insensitive, returns canonical case)', () => {
    expect(detectCredentialPrefix('Bearer abc123')).toBe('Bearer')
    expect(detectCredentialPrefix('bearer abc123')).toBe('Bearer')
    expect(detectCredentialPrefix('  Basic dXNlcjpwYXNz')).toBe('Basic')
    expect(detectCredentialPrefix('Token gho_xxx')).toBe('Token')
    expect(detectCredentialPrefix('Bot 123:ABC')).toBe('Bot')
    expect(detectCredentialPrefix('Digest xyz')).toBe('Digest')
  })

  it('does not flag a bare key', () => {
    expect(detectCredentialPrefix('sk-abc123')).toBeNull()
    expect(detectCredentialPrefix('gho_1234567890')).toBeNull()
    expect(detectCredentialPrefix('')).toBeNull()
  })

  it('does not flag substrings or scheme-word without a trailing value', () => {
    expect(detectCredentialPrefix('bearertoken-no-space')).toBeNull()
    expect(detectCredentialPrefix('my-bearer-key')).toBeNull()
    expect(detectCredentialPrefix('Bearer')).toBeNull()
    expect(detectCredentialPrefix('Bearer   ')).toBeNull()
  })
})

describe('evaluateCredentialSubmit', () => {
  it('proceeds for a bare key', () => {
    expect(evaluateCredentialSubmit('sk-abc', false)).toEqual({ proceed: true, warning: null })
  })

  it('blocks the first time a prefixed value is submitted and names the scheme', () => {
    const r = evaluateCredentialSubmit('Bearer sk-abc', false)
    expect(r.proceed).toBe(false)
    expect(r.warning).toContain('Bearer')
    expect(r.warning).toContain('bare key')
  })

  it('proceeds on the second submit (already warned) of the same prefixed value', () => {
    expect(evaluateCredentialSubmit('Bearer sk-abc', true)).toEqual({ proceed: true, warning: null })
  })
})

describe('provider composition labels', () => {
  it('shows composed state independently from provider health status', () => {
    expect(providerCompositionLabel(providerView({ composed: true }))).toBe('Composed')
    expect(providerCompositionClass(providerView({ composed: true }))).toBe('ok')

    expect(providerCompositionLabel(providerView({ composed: false }))).toBe('Not composed')
    expect(providerCompositionClass(providerView({ composed: false }))).toBe('bad')

    const degradedButComposed = providerView({
      composed: true,
      status: { kind: 'gateway_error', message: 'temporary lookup failure' },
    })
    expect(providerCompositionLabel(degradedButComposed)).toBe('Composed')
    expect(providerCompositionClass(degradedButComposed)).toBe('ok')
  })
})

describe('microcopy', () => {
  it('credential hint tells users to omit Bearer/header', () => {
    expect(CREDENTIAL_HINT).toContain('Bearer')
    expect(CREDENTIAL_HINT.toLowerCase()).toContain('only')
  })

  it('header-name hint explains the consumer must match it', () => {
    expect(HEADER_NAME_HINT).toContain('Authorization')
  })
})
