const LABELS: Record<string, string> = {
  sandbox: 'Live',
  not_authored: 'Not authored yet',
  host_mirror: 'Host mirror',
  sandbox_unreachable: 'Sandbox unreachable',
  host: 'Host',
  missing: 'Missing',
}

const TONES: Record<string, string> = {
  sandbox: 'ok',
  host: 'ok',
  host_mirror: 'muted',
  not_authored: 'muted',
  sandbox_unreachable: 'bad',
  missing: 'bad',
}

export function identityLabel(state: string): string {
  return LABELS[state] ?? state
}

export function identityTone(state: string): string {
  return TONES[state] ?? 'muted'
}
