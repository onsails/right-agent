import { beforeEach, describe, expect, test, vi } from 'vitest'
import type { Mock } from 'vitest'

import {
  applyTelegramDisplayMode,
  DASHBOARD_DISPLAY_MODE_STORAGE_KEY,
  initializeTelegramWebApp,
  nextDashboardDisplayModePreference,
  readDashboardDisplayMode,
  saveDashboardDisplayMode,
  subscribeTelegramFullscreenChanges,
  type TelegramWebApp,
} from './telegram'

type TestStorage = {
  getItem: Mock<(key: string) => string | null>
  setItem: Mock<(key: string, value: string) => void>
}

function storageWithValue(value: string | null): TestStorage {
  return {
    getItem: vi.fn(() => value),
    setItem: vi.fn(),
  }
}

function webAppWithFullscreen(isFullscreen = false): TelegramWebApp {
  return {
    isFullscreen,
    ready: vi.fn(),
    expand: vi.fn(),
    requestFullscreen: vi.fn(function (this: TelegramWebApp) {
      this.isFullscreen = true
    }),
    exitFullscreen: vi.fn(function (this: TelegramWebApp) {
      this.isFullscreen = false
    }),
  }
}

describe('Telegram dashboard display mode helpers', () => {
  const originalLocalStorageDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage')

  beforeEach(() => {
    if (originalLocalStorageDescriptor) {
      Object.defineProperty(globalThis, 'localStorage', originalLocalStorageDescriptor)
    } else {
      Reflect.deleteProperty(globalThis, 'localStorage')
    }
    vi.restoreAllMocks()
  })

  test('exports the dashboard display mode storage key', () => {
    expect(DASHBOARD_DISPLAY_MODE_STORAGE_KEY).toBe('right-dashboard.display-mode')
  })

  test('reads normal when no preference is stored', () => {
    expect(readDashboardDisplayMode(storageWithValue(null))).toBe('normal')
  })

  test('reads a saved fullscreen preference', () => {
    expect(readDashboardDisplayMode(storageWithValue('fullscreen'))).toBe('fullscreen')
  })

  test('normalizes invalid stored values to normal', () => {
    expect(readDashboardDisplayMode(storageWithValue('maximized'))).toBe('normal')
  })

  test('ignores storage read failures', () => {
    const storage = storageWithValue(null)
    storage.getItem.mockImplementation(() => {
      throw new Error('storage unavailable')
    })

    expect(readDashboardDisplayMode(storage)).toBe('normal')
  })

  test('defaults to normal when global storage acquisition fails during initialization', () => {
    const webApp = webAppWithFullscreen(false)
    Object.defineProperty(globalThis, 'localStorage', {
      configurable: true,
      get() {
        throw new Error('storage unavailable')
      },
    })

    expect(() => {
      expect(initializeTelegramWebApp(webApp)).toBe('normal')
    }).not.toThrow()
  })

  test('saves dashboard display mode preferences', () => {
    const storage = storageWithValue(null)

    saveDashboardDisplayMode('fullscreen', storage)

    expect(storage.setItem).toHaveBeenCalledWith(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, 'fullscreen')
  })

  test('ignores storage write failures', () => {
    const storage = storageWithValue(null)
    storage.setItem.mockImplementation(() => {
      throw new Error('storage unavailable')
    })

    expect(() => saveDashboardDisplayMode('fullscreen', storage)).not.toThrow()
  })

  test('initializes normal mode with ready and expand but not fullscreen', () => {
    const webApp = webAppWithFullscreen(false)

    const actualMode = initializeTelegramWebApp(webApp, 'normal')

    expect(actualMode).toBe('normal')
    expect(webApp.ready).toHaveBeenCalledOnce()
    expect(webApp.expand).toHaveBeenCalledOnce()
    expect(webApp.requestFullscreen).not.toHaveBeenCalled()
  })

  test('initializes fullscreen mode with ready, expand, and fullscreen request', () => {
    const webApp = webAppWithFullscreen(false)

    const actualMode = initializeTelegramWebApp(webApp, 'fullscreen')

    expect(actualMode).toBe('fullscreen')
    expect(webApp.ready).toHaveBeenCalledOnce()
    expect(webApp.expand).toHaveBeenCalledOnce()
    expect(webApp.requestFullscreen).toHaveBeenCalledOnce()
  })

  test('returns actual normal mode when initial fullscreen request fails synchronously', () => {
    const webApp = webAppWithFullscreen(false)
    vi.mocked(webApp.requestFullscreen!).mockImplementation(() => {
      throw new Error('fullscreen unavailable')
    })

    const actualMode = initializeTelegramWebApp(webApp, 'fullscreen')

    expect(actualMode).toBe('normal')
    expect(webApp.expand).toHaveBeenCalledOnce()
  })

  test('returns actual normal mode when initial fullscreen request is unsupported', () => {
    const webApp: TelegramWebApp = {
      ready: vi.fn(),
      expand: vi.fn(),
    }

    expect(() => {
      expect(initializeTelegramWebApp(webApp, 'fullscreen')).toBe('normal')
    }).not.toThrow()
  })

  test('applies fullscreen mode by saving preference and requesting fullscreen', () => {
    const webApp = webAppWithFullscreen(false)
    const storage = storageWithValue(null)

    const actualMode = applyTelegramDisplayMode('fullscreen', webApp, storage)

    expect(actualMode).toBe('fullscreen')
    expect(storage.setItem).toHaveBeenCalledWith(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, 'fullscreen')
    expect(webApp.requestFullscreen).toHaveBeenCalledOnce()
  })

  test('applies normal mode by saving preference and exiting fullscreen', () => {
    const webApp = webAppWithFullscreen(true)
    const storage = storageWithValue(null)

    const actualMode = applyTelegramDisplayMode('normal', webApp, storage)

    expect(actualMode).toBe('normal')
    expect(storage.setItem).toHaveBeenCalledWith(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, 'normal')
    expect(webApp.exitFullscreen).toHaveBeenCalledOnce()
  })

  test('keeps fullscreen preference but returns actual normal layout when fullscreen request fails', () => {
    const webApp = webAppWithFullscreen(false)
    const storage = storageWithValue(null)
    vi.mocked(webApp.requestFullscreen!).mockImplementation(() => {
      throw new Error('fullscreen unavailable')
    })

    const actualMode = applyTelegramDisplayMode('fullscreen', webApp, storage)

    expect(actualMode).toBe('normal')
    expect(storage.setItem).toHaveBeenCalledWith(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, 'fullscreen')
  })

  test('keeps fullscreen preference but returns actual normal layout when fullscreen request is unsupported', () => {
    const webApp: TelegramWebApp = {}
    const storage = storageWithValue(null)

    const actualMode = applyTelegramDisplayMode('fullscreen', webApp, storage)

    expect(actualMode).toBe('normal')
    expect(storage.setItem).toHaveBeenCalledWith(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, 'fullscreen')
  })

  test('derives the next toggle target from saved preference after fullscreen falls back to normal', () => {
    const webApp: TelegramWebApp = {}
    const storage = storageWithValue('fullscreen')
    const preferredMode = readDashboardDisplayMode(storage)
    const actualMode = applyTelegramDisplayMode(preferredMode, webApp, storage)
    const nextPreference = nextDashboardDisplayModePreference(preferredMode)

    applyTelegramDisplayMode(nextPreference, webApp, storage)

    expect(actualMode).toBe('normal')
    expect(storage.setItem).toHaveBeenLastCalledWith(DASHBOARD_DISPLAY_MODE_STORAGE_KEY, 'normal')
  })

  test('subscribes to fullscreen changes and unsubscribes with offEvent', () => {
    let handler: (() => void) | undefined
    const onChange = vi.fn()
    const webApp: TelegramWebApp = {
      isFullscreen: false,
      onEvent: vi.fn((eventName, nextHandler) => {
        expect(eventName).toBe('fullscreenChanged')
        handler = nextHandler
      }),
      offEvent: vi.fn((eventName, nextHandler) => {
        expect(eventName).toBe('fullscreenChanged')
        expect(nextHandler).toBe(handler)
      }),
    }

    const unsubscribe = subscribeTelegramFullscreenChanges(webApp, onChange)
    webApp.isFullscreen = true
    handler?.()
    webApp.isFullscreen = false
    handler?.()
    unsubscribe()

    expect(onChange).toHaveBeenNthCalledWith(1, 'fullscreen')
    expect(onChange).toHaveBeenNthCalledWith(2, 'normal')
    expect(webApp.offEvent).toHaveBeenCalledOnce()
  })

  test('ignores Telegram event subscription failures', () => {
    const webApp: TelegramWebApp = {
      onEvent: vi.fn(() => {
        throw new Error('event api unavailable')
      }),
    }

    expect(() => subscribeTelegramFullscreenChanges(webApp, vi.fn())).not.toThrow()
  })

  test('ignores Telegram event unsubscribe failures', () => {
    const webApp: TelegramWebApp = {
      onEvent: vi.fn(),
      offEvent: vi.fn(() => {
        throw new Error('event api unavailable')
      }),
    }

    const unsubscribe = subscribeTelegramFullscreenChanges(webApp, vi.fn())

    expect(() => unsubscribe()).not.toThrow()
  })
})
