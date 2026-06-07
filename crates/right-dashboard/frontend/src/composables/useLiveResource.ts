import { computed, onBeforeUnmount, onMounted, ref, watch, type Ref } from 'vue'

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

export function settledBlockingGeneration(blockingGeneration: number | null, settledGeneration: number): number | null {
  return blockingGeneration === settledGeneration ? null : blockingGeneration
}

export interface LiveResourceOptions {
  intervalMs?: number
  immediate?: boolean
  pauseWhenHidden?: boolean
  reportConnection?: boolean
  key?: string
}

export interface LiveResourceRefreshOptions {
  force?: boolean
  reset?: boolean
}

export interface LiveResource<T> {
  data: Ref<T | null>
  error: Ref<string | null>
  loading: Ref<boolean>
  lastUpdatedAt: Ref<string | null>
  refresh: (options?: LiveResourceRefreshOptions) => Promise<void>
}

let keySeq = 0

export function useLiveResource<T>(fetcher: () => Promise<T>, options: LiveResourceOptions = {}): LiveResource<T> {
  const config = useLiveConfig()
  const intervalMs = computed(() => options.intervalMs ?? config.value.intervalMs)
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
  let blockingGeneration: number | null = null
  let generation = 0
  let timer: ReturnType<typeof window.setInterval> | undefined
  let stopIntervalWatch: (() => void) | undefined

  function hasInFlight(): boolean {
    return blockingGeneration !== null
  }

  async function refresh(options: LiveResourceRefreshOptions = {}): Promise<void> {
    if (disposed || (hasInFlight() && !options.force)) {
      return
    }
    if (options.reset) {
      data.value = null
      error.value = null
    }
    const gen = ++generation
    blockingGeneration = gen
    // Only surface `loading` on the initial fetch (no data yet). Background
    // polls must not toggle it: views gate content behind `loading` via
    // AsyncState, and flipping it every interval would unmount the content
    // slot, collapse the scroll height, and reset the user's scroll to top.
    if (data.value === null) {
      loading.value = true
    }
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
      blockingGeneration = settledBlockingGeneration(blockingGeneration, gen)
    }
  }

  function onVisibility(): void {
    if (!document.hidden) {
      void refresh()
    }
  }

  function startTimer(): void {
    if (timer !== undefined) {
      window.clearInterval(timer)
      timer = undefined
    }
    if (intervalMs.value > 0) {
      timer = window.setInterval(() => {
        if (shouldTick({ hidden: document.hidden, inFlight: hasInFlight(), pauseWhenHidden })) {
          void refresh()
        }
      }, intervalMs.value)
    }
  }

  onMounted(() => {
    if (immediate) {
      void refresh()
    }
    startTimer()
    stopIntervalWatch = watch(intervalMs, startTimer)
    if (pauseWhenHidden) {
      document.addEventListener('visibilitychange', onVisibility)
    }
  })

  onBeforeUnmount(() => {
    disposed = true
    stopIntervalWatch?.()
    if (timer !== undefined) {
      window.clearInterval(timer)
    }
    document.removeEventListener('visibilitychange', onVisibility)
    status?.dispose()
  })

  return { data, error, loading, lastUpdatedAt, refresh }
}
