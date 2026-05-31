import { onBeforeUnmount, onMounted, ref, type Ref } from 'vue'

import { classifyOutcome, registerLiveResource } from './liveStatus'
import { useLiveConfig } from './liveConfig'

export function shouldTick(state: { hidden: boolean, inFlight: boolean, pauseWhenHidden: boolean }): boolean {
  if (state.inFlight) {
    return false
  }
  if (state.pauseWhenHidden && state.hidden) {
    return false
  }
  return true
}

export interface LiveResourceOptions {
  intervalMs?: number
  immediate?: boolean
  pauseWhenHidden?: boolean
  reportConnection?: boolean
  key?: string
}

export interface LiveResource<T> {
  data: Ref<T | null>
  error: Ref<string | null>
  loading: Ref<boolean>
  lastUpdatedAt: Ref<string | null>
  refresh: () => Promise<void>
}

let keySeq = 0

export function useLiveResource<T>(fetcher: () => Promise<T>, options: LiveResourceOptions = {}): LiveResource<T> {
  const config = useLiveConfig()
  const intervalMs = options.intervalMs ?? config.intervalMs
  const immediate = options.immediate ?? true
  const pauseWhenHidden = options.pauseWhenHidden ?? true
  const reportConnection = options.reportConnection ?? true
  const key = options.key ?? `live-${keySeq++}`

  const data = ref(null) as Ref<T | null>
  const error = ref<string | null>(null)
  const loading = ref(false)
  const lastUpdatedAt = ref<string | null>(null)

  const status = reportConnection ? registerLiveResource(key) : null
  let disposed = false
  let inFlight = false
  let generation = 0
  let timer: ReturnType<typeof window.setInterval> | undefined

  async function refresh(): Promise<void> {
    if (disposed || inFlight) {
      return
    }
    inFlight = true
    loading.value = true
    const gen = ++generation
    try {
      const result = await fetcher()
      if (disposed || gen !== generation) {
        return
      }
      data.value = result
      error.value = null
      const at = new Date().toISOString()
      lastUpdatedAt.value = at
      status?.report('live', at)
    } catch (err) {
      if (disposed || gen !== generation) {
        return
      }
      const hasData = data.value !== null
      if (!hasData) {
        error.value = err instanceof Error ? err.message : 'Request failed'
      }
      status?.report(classifyOutcome({ ok: false, error: err, hasData }))
    } finally {
      if (!disposed && gen === generation) {
        loading.value = false
      }
      inFlight = false
    }
  }

  function onVisibility(): void {
    if (!document.hidden) {
      void refresh()
    }
  }

  onMounted(() => {
    if (immediate) {
      void refresh()
    }
    if (intervalMs > 0) {
      timer = window.setInterval(() => {
        if (shouldTick({ hidden: document.hidden, inFlight, pauseWhenHidden })) {
          void refresh()
        }
      }, intervalMs)
    }
    if (pauseWhenHidden) {
      document.addEventListener('visibilitychange', onVisibility)
    }
  })

  onBeforeUnmount(() => {
    disposed = true
    if (timer !== undefined) {
      window.clearInterval(timer)
    }
    document.removeEventListener('visibilitychange', onVisibility)
    status?.dispose()
  })

  return { data, error, loading, lastUpdatedAt, refresh }
}
