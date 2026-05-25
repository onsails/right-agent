export type DashboardDisplayMode = 'normal' | 'fullscreen'

export const DASHBOARD_DISPLAY_MODE_STORAGE_KEY = 'right-dashboard.display-mode'

type DashboardDisplayModeStorage = Pick<Storage, 'getItem' | 'setItem'>
type TelegramFullscreenChangedHandler = () => void

export interface TelegramWebApp {
  initData?: string
  ready?: () => void
  requestFullscreen?: () => void
  exitFullscreen?: () => void
  expand?: () => void
  isFullscreen?: boolean
  onEvent?: (eventType: 'fullscreenChanged', eventHandler: TelegramFullscreenChangedHandler) => void
  offEvent?: (eventType: 'fullscreenChanged', eventHandler: TelegramFullscreenChangedHandler) => void
}

declare global {
  interface Window {
    Telegram?: {
      WebApp?: TelegramWebApp
    }
  }
}

function defaultWebApp(): TelegramWebApp | undefined {
  if (typeof window === 'undefined') {
    return undefined
  }
  return window.Telegram?.WebApp
}

function defaultStorage(): DashboardDisplayModeStorage | undefined {
  try {
    if (typeof localStorage === 'undefined') {
      return undefined
    }
    return localStorage
  } catch {
    return undefined
  }
}

function normalizedDisplayMode(value: string | null): DashboardDisplayMode {
  return value === 'fullscreen' ? 'fullscreen' : 'normal'
}

function actualDisplayMode(webApp: TelegramWebApp | undefined, fallbackMode: DashboardDisplayMode): DashboardDisplayMode {
  if (typeof webApp?.isFullscreen === 'boolean') {
    return webApp.isFullscreen ? 'fullscreen' : 'normal'
  }
  return fallbackMode
}

export function readDashboardDisplayMode(storage: DashboardDisplayModeStorage | undefined = defaultStorage()): DashboardDisplayMode {
  try {
    return normalizedDisplayMode(storage?.getItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY) ?? null)
  } catch {
    return 'normal'
  }
}

export function saveDashboardDisplayMode(
  mode: DashboardDisplayMode,
  storage: DashboardDisplayModeStorage | undefined = defaultStorage(),
): void {
  try {
    storage?.setItem(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, mode)
  } catch {
    // Storage may be unavailable or blocked; Telegram display changes should still proceed.
  }
}

export function initializeTelegramWebApp(
  webApp: TelegramWebApp | undefined = defaultWebApp(),
  preferredMode: DashboardDisplayMode = readDashboardDisplayMode(),
): DashboardDisplayMode {
  webApp?.ready?.()
  webApp?.expand?.()

  if (preferredMode !== 'fullscreen') {
    return actualDisplayMode(webApp, 'normal')
  }

  try {
    webApp?.requestFullscreen?.()
  } catch {
    return actualDisplayMode(webApp, 'normal')
  }

  return actualDisplayMode(webApp, 'fullscreen')
}

export function applyTelegramDisplayMode(
  mode: DashboardDisplayMode,
  webApp: TelegramWebApp | undefined = defaultWebApp(),
  storage: DashboardDisplayModeStorage | undefined = defaultStorage(),
): DashboardDisplayMode {
  saveDashboardDisplayMode(mode, storage)

  try {
    if (mode === 'fullscreen') {
      webApp?.requestFullscreen?.()
    } else {
      webApp?.exitFullscreen?.()
    }
  } catch {
    return actualDisplayMode(webApp, 'normal')
  }

  return actualDisplayMode(webApp, mode)
}

export function subscribeTelegramFullscreenChanges(
  webApp: TelegramWebApp | undefined,
  onChange: (mode: DashboardDisplayMode) => void,
): () => void {
  const handler: TelegramFullscreenChangedHandler = () => {
    onChange(webApp?.isFullscreen ? 'fullscreen' : 'normal')
  }

  try {
    webApp?.onEvent?.('fullscreenChanged', handler)
  } catch {
    // Telegram event APIs vary by client; display mode tracking is opportunistic.
  }

  return () => {
    try {
      webApp?.offEvent?.('fullscreenChanged', handler)
    } catch {
      // Cleanup must not break dashboard teardown when a client rejects offEvent.
    }
  }
}
