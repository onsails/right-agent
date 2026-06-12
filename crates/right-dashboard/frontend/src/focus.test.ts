import { describe, expect, it } from 'vitest'

import { focusLaunchParams } from './focus'

describe('focusLaunchParams', () => {
  it('parses focus view launch params', () => {
    expect(focusLaunchParams('?view=focus&chat_id=-100123&thread_id=7&token=abc.123')).toEqual({
      chatId: -100123,
      threadId: 7,
      token: 'abc.123',
    })
  })

  it('returns null when view is not focus', () => {
    expect(focusLaunchParams('?view=providers&chat_id=-100123&thread_id=7')).toBeNull()
  })

  it('returns null when chat_id is missing or zero', () => {
    expect(focusLaunchParams('?view=focus&thread_id=7')).toBeNull()
    expect(focusLaunchParams('?view=focus&chat_id=0&thread_id=7')).toBeNull()
  })

  it('accepts thread_id=0', () => {
    expect(focusLaunchParams('?view=focus&chat_id=42&thread_id=0&token=abc.123')).toEqual({
      chatId: 42,
      threadId: 0,
      token: 'abc.123',
    })
  })

  it('returns null when thread_id or token is missing', () => {
    expect(focusLaunchParams('?view=focus&chat_id=42')).toBeNull()
    expect(focusLaunchParams('?view=focus&chat_id=42&thread_id=7')).toBeNull()
  })

  it('returns null for malformed number parameters', () => {
    expect(focusLaunchParams('?view=focus&chat_id=abc&thread_id=7')).toBeNull()
    expect(focusLaunchParams('?view=focus&chat_id=42&thread_id=1.5')).toBeNull()
  })

  it('returns null for non-decimal chat_id values', () => {
    expect(focusLaunchParams('?view=focus&chat_id=1e3&thread_id=7')).toBeNull()
    expect(focusLaunchParams('?view=focus&chat_id=0x10&thread_id=7')).toBeNull()
  })

  it('returns null for whitespace-padded chat_id values', () => {
    expect(focusLaunchParams('?view=focus&chat_id=%2042%20&thread_id=7')).toBeNull()
  })

  it('returns null for unsafe integer chat_id values', () => {
    expect(focusLaunchParams('?view=focus&chat_id=9007199254740992&thread_id=7')).toBeNull()
  })

  it('returns null for negative thread_id values', () => {
    expect(focusLaunchParams('?view=focus&chat_id=42&thread_id=-1')).toBeNull()
  })

  it('returns null for non-decimal thread_id values', () => {
    expect(focusLaunchParams('?view=focus&chat_id=42&thread_id=1e3')).toBeNull()
    expect(focusLaunchParams('?view=focus&chat_id=42&thread_id=0x10')).toBeNull()
  })

  it('returns null for whitespace-padded thread_id values', () => {
    expect(focusLaunchParams('?view=focus&chat_id=42&thread_id=%207%20')).toBeNull()
  })

  it('returns null for unsafe integer thread_id values', () => {
    expect(focusLaunchParams('?view=focus&chat_id=42&thread_id=9007199254740992')).toBeNull()
  })
})
