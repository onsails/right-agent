export type DashboardDisplayMode = 'normal' | 'fullscreen'

export const DASHBOARD_DISPLAY_MODE_STORAGE_KEY = 'right-dashboard.display-mode'

const FULLSCREEN_CHANGED_EVENT = 'fullscreenChanged'
type FullscreenChangedEvent = typeof FULLSCREEN_CHANGED_EVENT

type DashboardDisplayModeStorage = Pick<Storage, 'getItem' | 'setItem'>
type TelegramFullscreenChangedHandler = () => void

export interface TelegramWebApp {
  initData?: string
  ready?: () => void
  requestFullscreen?: () => void
  exitFullscreen?: () => void
  expand?: () => void
  isFullscreen?: boolean
  onEvent?: (eventType: FullscreenChangedEvent, eventHandler: TelegramFullscreenChangedHandler) => void
  offEvent?: (eventType: FullscreenChangedEvent, eventHandler: TelegramFullscreenChangedHandler) => void
  openLink?: (url: string) => void
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
    /* empty */
  }
}

export function nextDashboardDisplayModePreference(mode: DashboardDisplayMode): DashboardDisplayMode {
  return mode === 'fullscreen' ? 'normal' : 'fullscreen'
}

// Telegram's requestFullscreen/exitFullscreen are async — the new state arrives via fullscreenChanged.
// Return the requested mode optimistically; subscribeTelegramFullscreenChanges corrects if the client denies.
function tryRequestFullscreen(webApp: TelegramWebApp | undefined): DashboardDisplayMode {
  if (typeof webApp?.requestFullscreen !== 'function') {
    return actualDisplayMode(webApp, 'normal')
  }
  try {
    webApp.requestFullscreen()
    return 'fullscreen'
  } catch {
    return actualDisplayMode(webApp, 'normal')
  }
}

function tryExitFullscreen(webApp: TelegramWebApp | undefined): DashboardDisplayMode {
  try {
    webApp?.exitFullscreen?.()
    return 'normal'
  } catch {
    return actualDisplayMode(webApp, 'fullscreen')
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
  return tryRequestFullscreen(webApp)
}

export function applyTelegramDisplayMode(
  mode: DashboardDisplayMode,
  webApp: TelegramWebApp | undefined = defaultWebApp(),
  storage: DashboardDisplayModeStorage | undefined = defaultStorage(),
): DashboardDisplayMode {
  saveDashboardDisplayMode(mode, storage)
  return mode === 'fullscreen' ? tryRequestFullscreen(webApp) : tryExitFullscreen(webApp)
}

export function subscribeTelegramFullscreenChanges(
  webApp: TelegramWebApp | undefined,
  onChange: (mode: DashboardDisplayMode) => void,
): () => void {
  const handler: TelegramFullscreenChangedHandler = () => {
    onChange(webApp?.isFullscreen ? 'fullscreen' : 'normal')
  }

  try {
    webApp?.onEvent?.(FULLSCREEN_CHANGED_EVENT, handler)
  } catch {
    /* empty */
  }

  return () => {
    try {
      webApp?.offEvent?.(FULLSCREEN_CHANGED_EVENT, handler)
    } catch {
      /* empty */
    }
  }
}
