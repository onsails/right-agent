export function learningSignalLabel(kind: string): string {
  switch (kind) {
    case 'skill_created':
      return 'Created'
    case 'skill_updated':
      return 'Updated'
    case 'skill_refused':
      return 'Refused'
    case 'skill_failed':
      return 'Failed'
    case 'skill_aborted':
      return 'Aborted'
    default:
      return 'Learned'
  }
}
