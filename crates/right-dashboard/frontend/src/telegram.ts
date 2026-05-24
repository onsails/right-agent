export interface TelegramWebApp {
  initData?: string
  ready?: () => void
  requestFullscreen?: () => void
  expand?: () => void
}

declare global {
  interface Window {
    Telegram?: {
      WebApp?: TelegramWebApp
    }
  }
}

export function initializeTelegramWebApp(webApp: TelegramWebApp | undefined = window.Telegram?.WebApp): void {
  webApp?.ready?.()
  try {
    webApp?.requestFullscreen?.()
  } catch {
    // Fullscreen is an optional Telegram client capability; keep the dashboard usable.
  }
  webApp?.expand?.()
}
