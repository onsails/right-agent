/**
 * Label for a capped failure sample, or `null` when every failure is shown.
 * The badge already carries the exact total; this only annotates the list when
 * it is a newest-first sample (`total > shown`).
 */
export function failureSampleLabel(total: number, shown: number): string | null {
  if (total > shown) {
    return `latest ${shown} of ${total}`
  }
  return null
}
