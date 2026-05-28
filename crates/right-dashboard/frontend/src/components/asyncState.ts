export type AsyncStateKind = 'error' | 'loading' | 'empty' | 'content'

export interface AsyncStateInput {
  loading: boolean
  error: string | null
  empty: boolean
}

/**
 * Decide what an async panel should render. Error wins over loading so a
 * failed refresh is never masked by a spinner; loading wins over empty so a
 * not-yet-loaded panel never flashes "nothing here".
 */
export function resolveAsyncState(input: AsyncStateInput): AsyncStateKind {
  if (input.error) {
    return 'error'
  }
  if (input.loading) {
    return 'loading'
  }
  if (input.empty) {
    return 'empty'
  }
  return 'content'
}
