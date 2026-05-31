export interface FailureMetric {
  tone: 'default' | 'bad'
  interactive: boolean
}

/** A failure count is calm (gray, inert) at zero and red+clickable above it. */
export function failureMetric(count: number): FailureMetric {
  if (count > 0) {
    return { tone: 'bad', interactive: true }
  }
  return { tone: 'default', interactive: false }
}
