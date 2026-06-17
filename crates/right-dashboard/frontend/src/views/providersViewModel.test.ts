import { describe, expect, it } from 'vitest'

import {
  borrowedOwnerLabel,
  detectCredentialPrefix,
  evaluateCredentialSubmit,
  isBorrowed,
  providerCompositionClass,
  providerCompositionLabel,
  shareTargetState,
  validateUpstreamHosts,
  CREDENTIAL_HINT,
  HOSTS_MICROCOPY,
} from './providersViewModel'
import type { ProviderPeer, ProviderView } from '../types'

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

    expect(providerCompositionLabel(providerView({ composed: null }))).toBe('Unknown')
    expect(providerCompositionClass(providerView({ composed: null }))).toBe('warn')

    const degradedButComposed = providerView({
      composed: true,
      status: { kind: 'gateway_error', message: 'temporary lookup failure' },
    })
    expect(providerCompositionLabel(degradedButComposed)).toBe('Composed')
    expect(providerCompositionClass(degradedButComposed)).toBe('ok')
  })
})

describe('validateUpstreamHosts', () => {
  it('requires at least one non-empty host', () => {
    expect(validateUpstreamHosts([])).toBe('required')
    expect(validateUpstreamHosts(['', '   '])).toBe('required')
  })

  it('accepts any trimmed host in the list', () => {
    expect(validateUpstreamHosts(['', ' api.openai.com '])).toBeNull()
  })
})

describe('microcopy', () => {
  it('credential hint tells users to omit Bearer/header', () => {
    expect(CREDENTIAL_HINT).toContain('Bearer')
    expect(CREDENTIAL_HINT.toLowerCase()).toContain('only')
  })

  it('hosts microcopy explains env-var auth and allowed hosts', () => {
    expect(HOSTS_MICROCOPY.toLowerCase()).toContain('hosts')
    expect(HOSTS_MICROCOPY).toContain('$ENV_VAR')
    expect(HOSTS_MICROCOPY).toContain('API docs')
    expect(HOSTS_MICROCOPY).toContain('Right stores')
  })
})

describe('isBorrowed / borrowedOwnerLabel', () => {
  it('treats an absent or empty shared_from as owned', () => {
    expect(isBorrowed(providerView())).toBe(false)
    expect(isBorrowed(providerView({ shared_from: null }))).toBe(false)
    expect(isBorrowed(providerView({ shared_from: '' }))).toBe(false)
    expect(borrowedOwnerLabel(providerView())).toBeNull()
  })

  it('treats a non-empty shared_from as borrowed and builds the owner label', () => {
    const p = providerView({ shared_from: 'agent-a' })
    expect(isBorrowed(p)).toBe(true)
    expect(borrowedOwnerLabel(p)).toBe('Shared from agent-a')
  })
})

describe('shareTargetState', () => {
  function peer(overrides: Partial<ProviderPeer> = {}): ProviderPeer {
    return { agent: 'agent-a', network_policy: 'permissive', providers: [], ...overrides }
  }

  it('allows sharing an owned provider to a peer that lacks it', () => {
    const p = providerView({ name: 'fal', generic: null })
    expect(shareTargetState(peer(), p).blocked).toBeNull()
  })

  it('blocks when the peer already has a provider with the same name', () => {
    const p = providerView({ name: 'fal', generic: null })
    const target = peer({
      providers: [{ name: 'fal', type: 'right-fal', env_var: 'FAL_KEY', label: null, generic: null }],
    })
    expect(shareTargetState(target, p).blocked).toBe('already shared with this agent')
  })

  it('blocks a generic provider when the peer is restrictive', () => {
    const p = providerView({ name: 'fal', generic: { env_var: 'FAL_KEY', upstream_hosts: ['fal.run'] } })
    const state = shareTargetState(peer({ network_policy: 'restrictive' }), p)
    expect(state.blocked).toBe('restrictive policy cannot accept generic providers')
  })

  it('allows a non-generic provider to a restrictive peer', () => {
    const p = providerView({ name: 'gh', generic: null })
    expect(shareTargetState(peer({ network_policy: 'restrictive' }), p).blocked).toBeNull()
  })
})
