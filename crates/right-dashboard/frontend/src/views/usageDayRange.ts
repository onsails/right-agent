const MONTH = new Intl.DateTimeFormat('en-US', { month: 'short', timeZone: 'UTC' })

function pad2(value: number): string {
  return String(value).padStart(2, '0')
}

function localParts(date: Date, timezone: string): { year: number; month: number; day: number; hour: number; minute: number } {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone: timezone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(date)

  const get = (type: string) => Number(parts.find((part) => part.type === type)?.value ?? 0)
  return {
    year: get('year'),
    month: get('month'),
    day: get('day'),
    hour: get('hour'),
    minute: get('minute'),
  }
}

function monthName(month: number): string {
  return MONTH.format(new Date(Date.UTC(2026, month - 1, 1)))
}

export function selectedDayRangeLabel(
  selectedDate: string | null,
  timezone: string,
  generatedAt: string,
): string | null {
  if (selectedDate === null) {
    return null
  }

  const generated = localParts(new Date(generatedAt), timezone)
  const currentDate = `${generated.year}-${pad2(generated.month)}-${pad2(generated.day)}`
  const [, monthRaw, dayRaw] = selectedDate.split('-')
  const month = Number(monthRaw)
  const day = Number(dayRaw)
  const end = selectedDate === currentDate ? `${pad2(generated.hour)}:${pad2(generated.minute)}` : '23:59'

  return `${timezone} · ${monthName(month)} ${day} 00:00-${end}`
}
