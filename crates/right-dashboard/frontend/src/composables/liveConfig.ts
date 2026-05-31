import { inject, provide, type InjectionKey, type Ref } from 'vue'

export const DEFAULT_INTERVAL_MS = 5000

export interface LiveConfig {
  intervalMs: number
}

const LiveConfigKey: InjectionKey<Ref<LiveConfig>> = Symbol('liveConfig')

export function provideLiveConfig(config: Ref<LiveConfig>): void {
  provide(LiveConfigKey, config)
}

export function useLiveConfig(): LiveConfig {
  const injected = inject(LiveConfigKey, null)
  return injected !== null ? injected.value : { intervalMs: DEFAULT_INTERVAL_MS }
}
