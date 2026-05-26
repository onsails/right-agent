export function money(value: number | null | undefined): string {
  return `$${(value ?? 0).toFixed(2)}`
}

export function percent(value: number | null | undefined): string {
  if (value === null || value === undefined) {
    return 'none'
  }
  return `${Math.round(value * 100)}%`
}

export function shortDate(value: string | null | undefined): string {
  if (!value) {
    return 'none'
  }
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) {
    return value
  }
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date)
}

export function shortId(id: string): string {
  return id.length > 10 ? `${id.slice(0, 8)}...` : id
}

export function bytes(value: number | null | undefined): string {
  if (value === null || value === undefined) {
    return 'none'
  }
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let next = value
  let unit = 0
  while (next >= 1024 && unit < units.length - 1) {
    next /= 1024
    unit += 1
  }
  return `${next >= 10 || unit === 0 ? next.toFixed(0) : next.toFixed(1)} ${units[unit]}`
}

export function initialDashboardTabFromLocation(search: string, hash: string): string {
  const params = new URLSearchParams(search)
  const queryView = params.get('view')
  if (queryView !== null && queryView.length > 0) {
    return queryView
  }
  const hashView = hash.replace(/^#/, '')
  return hashView.length > 0 ? hashView : 'overview'
}

export function statusTone(status: string | null | undefined): string {
  const normalized = (status ?? '').toLowerCase()
  if (
    normalized === 'success' ||
    normalized === 'delivered' ||
    normalized === 'pass' ||
    normalized === 'configured' ||
    normalized === 'sandbox' ||
    normalized === 'host' ||
    normalized === 'host_mirror' ||
    normalized === 'create_candidate' ||
    normalized === 'update_candidate'
  ) {
    return 'ok'
  }
  if (
    normalized === 'failed' ||
    normalized === 'fail' ||
    normalized === 'error' ||
    normalized === 'unavailable'
  ) {
    return 'bad'
  }
  if (
    normalized === 'queued' ||
    normalized === 'running' ||
    normalized === 'pending' ||
    normalized === 'warn' ||
    normalized === 'mixed' ||
    normalized === 'not_loaded'
  ) {
    return 'active'
  }
  return 'muted'
}

interface DeliveryDisplay {
  delivery_required: boolean
  delivery_status: string
  delivery_kind: string | null
}

export function deliveryText(run: DeliveryDisplay): string {
  const kind = run.delivery_kind?.trim().toLowerCase()
  if (kind === 'notify') {
    return 'Notify'
  }
  if (kind === 'silent') {
    return 'Silent'
  }
  if (kind) {
    return kind
  }
  return run.delivery_required ? 'Notify' : 'Silent'
}

export function deliveryLabel(run: DeliveryDisplay): string {
  const text = deliveryText(run)
  const status = run.delivery_status.trim().toLowerCase()
  if (!status || status === 'none') {
    return text
  }
  return `${text} ${status}`
}

export function deliveryTone(run: DeliveryDisplay): string {
  const status = run.delivery_status.trim().toLowerCase()
  if (status === 'delivered') {
    return 'ok'
  }
  if (status === 'failed') {
    return 'bad'
  }
  if (run.delivery_required || status === 'pending' || status === 'retryable') {
    return 'active'
  }
  return 'muted'
}

export function notifyText(value: unknown): string | null {
  if (value === null || value === undefined) {
    return null
  }
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}
