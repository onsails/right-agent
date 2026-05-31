import { describe, expect, it } from 'vitest'

import { failureMetric } from './failureMetric'

describe('failureMetric', () => {
  it('is gray and inert at zero', () => {
    expect(failureMetric(0)).toEqual({ tone: 'default', interactive: false })
  })
  it('is red and interactive when positive', () => {
    expect(failureMetric(3)).toEqual({ tone: 'bad', interactive: true })
  })
  it('treats negative counts as zero-like', () => {
    expect(failureMetric(-1)).toEqual({ tone: 'default', interactive: false })
  })
})
