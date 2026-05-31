import { inject, provide, ref, type InjectionKey, type Ref } from 'vue'

const DEFAULT_INTERVAL_MS = 5000

export interface LiveConfig {
  intervalMs: number
}

const LiveConfigKey: InjectionKey<Ref<LiveConfig>> = Symbol('liveConfig')

export function provideLiveConfig(config: Ref<LiveConfig>): void {
  provide(LiveConfigKey, config)
}

export function useLiveConfig(): Ref<LiveConfig> {
  const injected = inject(LiveConfigKey, null)
  return injected ?? ref({ intervalMs: DEFAULT_INTERVAL_MS })
}
