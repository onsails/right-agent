import { computed, reactive, ref } from 'vue'

import { DashboardApiError } from '../api'

export type ConnectionState = 'loading' | 'live' | 'stale' | 'offline' | 'locked'

const PRIORITY: ConnectionState[] = ['locked', 'offline', 'stale', 'loading', 'live']

export function reduceConnectionState(states: ConnectionState[]): ConnectionState | null {
  if (states.length === 0) {
    return null
  }
  for (const candidate of PRIORITY) {
    if (states.includes(candidate)) {
      return candidate
    }
  }
  return 'live'
}

export function classifyOutcome(args: { ok: boolean, error?: unknown, hasData: boolean }): ConnectionState {
  if (args.ok) {
    return 'live'
  }
  if (args.error instanceof DashboardApiError && args.error.isLocked) {
    return 'locked'
  }
  return args.hasData ? 'stale' : 'offline'
}

const registry = reactive(new Map<string, ConnectionState>())

export const globalLastUpdatedAt = ref<string | null>(null)

export const globalConnectionState = computed<ConnectionState>(() => {
  return reduceConnectionState([...registry.values()]) ?? 'loading'
})

export interface LiveStatusHandle {
  report: (state: ConnectionState, at?: string) => void
  dispose: () => void
}

export function registerLiveResource(key: string): LiveStatusHandle {
  registry.set(key, 'loading')
  return {
    report(state: ConnectionState, at?: string): void {
      registry.set(key, state)
      if (state === 'live' && at !== undefined) {
        globalLastUpdatedAt.value = at
      }
    },
    dispose(): void {
      registry.delete(key)
    },
  }
}
