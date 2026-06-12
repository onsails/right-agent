export interface FocusLaunch {
  chatId: number
  threadId: number
  token: string
}

export function focusLaunchParams(search: string): FocusLaunch | null {
  const params = new URLSearchParams(search)
  if (params.get('view') !== 'focus') {
    return null
  }

  const rawChatId = params.get('chat_id')
  const rawThreadId = params.get('thread_id')
  const token = params.get('token')
  if (rawChatId === null || rawThreadId === null || token === null || token === '') {
    return null
  }

  const chatId = parseSignedDecimalInteger(rawChatId)
  const threadId = parseUnsignedDecimalInteger(rawThreadId)
  if (chatId === null || chatId === 0 || threadId === null) {
    return null
  }

  return { chatId, threadId, token }
}

function parseSignedDecimalInteger(raw: string): number | null {
  if (!/^-?\d+$/.test(raw)) {
    return null
  }
  const value = Number(raw)
  return Number.isSafeInteger(value) ? value : null
}

function parseUnsignedDecimalInteger(raw: string): number | null {
  if (!/^\d+$/.test(raw)) {
    return null
  }
  const value = Number(raw)
  return Number.isSafeInteger(value) ? value : null
}
