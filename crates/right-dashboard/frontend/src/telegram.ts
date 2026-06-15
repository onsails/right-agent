export type DashboardDisplayMode = 'normal' | 'fullscreen'

export const DASHBOARD_DISPLAY_MODE_STORAGE_KEY = 'right-dashboard.display-mode'

const FULLSCREEN_CHANGED_EVENT = 'fullscreenChanged'
type FullscreenChangedEvent = typeof FULLSCREEN_CHANGED_EVENT

type DashboardDisplayModeStorage = Pick<Storage, 'getItem' | 'setItem'>
type TelegramFullscreenChangedHandler = () => void

const JEWEL_THEME_VARS: ReadonlyArray<[string, string]> = [
  ['--tg-theme-bg-color', 'var(--jewel-base)'],
  ['--tg-theme-secondary-bg-color', 'var(--jewel-panel)'],
  ['--tg-theme-text-color', 'var(--jewel-text)'],
  ['--tg-theme-hint-color', 'var(--jewel-muted)'],
  ['--tg-theme-hint_color', 'var(--jewel-muted)'],
  ['--tg-theme-link-color', 'var(--jewel-teal)'],
  ['--tg-theme-button_color', 'var(--jewel-teal)'],
  ['--tg-theme-section_separator_color', 'var(--jewel-line)'],
]

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
  showConfirm?: (message: string, callback: (confirmed: boolean) => void) => void
  showAlert?: (message: string, callback?: () => void) => void
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

function defaultConfirmFn(): ((message?: string) => boolean) | undefined {
  return typeof window === 'undefined' ? undefined : window.confirm.bind(window)
}

function defaultAlertFn(): ((message?: string) => void) | undefined {
  return typeof window === 'undefined' ? undefined : window.alert.bind(window)
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
  applyJewelTheme()

  if (preferredMode !== 'fullscreen') {
    return actualDisplayMode(webApp, 'normal')
  }
  return tryRequestFullscreen(webApp)
}

export function applyJewelTheme(
  root: HTMLElement | undefined = typeof document === 'undefined' ? undefined : document.documentElement,
): void {
  if (!root) {
    return
  }
  for (const [name, value] of JEWEL_THEME_VARS) {
    root.style.setProperty(name, value)
  }
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

/** Native Mini-App confirmation; falls back to `window.confirm`, else `false`. */
export function confirmAction(
  message: string,
  webApp: TelegramWebApp | undefined = defaultWebApp(),
  confirmFn: ((message?: string) => boolean) | undefined = defaultConfirmFn(),
): Promise<boolean> {
  if (typeof webApp?.showConfirm === 'function') {
    return new Promise((resolve) => webApp.showConfirm!(message, resolve))
  }
  return Promise.resolve(confirmFn ? confirmFn(message) : false)
}

/** Native Mini-App alert; falls back to `window.alert`, else no-op. */
export function alertMessage(
  message: string,
  webApp: TelegramWebApp | undefined = defaultWebApp(),
  alertFn: ((message?: string) => void) | undefined = defaultAlertFn(),
): Promise<void> {
  if (typeof webApp?.showAlert === 'function') {
    return new Promise((resolve) => webApp.showAlert!(message, resolve))
  }
  alertFn?.(message)
  return Promise.resolve()
}
