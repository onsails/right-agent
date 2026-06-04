import { describe, expect, it } from 'vitest'

import { learningSignalLabel } from './learningSignalLabel'

describe('learningSignalLabel', () => {
  it.each([
    ['skill_created', 'Created'],
    ['skill_updated', 'Updated'],
    ['skill_refused', 'Refused'],
    ['skill_failed', 'Failed'],
    ['skill_aborted', 'Aborted'],
    ['skill_learned', 'Learned'],
    ['something_unexpected', 'Learned'],
  ])('maps %s to %s', (kind, label) => {
    expect(learningSignalLabel(kind)).toBe(label)
  })
})
