import { describe, expect, test, vi } from 'vitest'

import { initializeTelegramWebApp } from './telegram'

describe('initializeTelegramWebApp', () => {
  test('requests fullscreen before expanding the dashboard web app', () => {
    const calls: string[] = []
    const webApp = {
      ready: vi.fn(() => calls.push('ready')),
      requestFullscreen: vi.fn(() => calls.push('requestFullscreen')),
      expand: vi.fn(() => calls.push('expand')),
    }

    initializeTelegramWebApp(webApp)

    expect(calls).toEqual(['ready', 'requestFullscreen', 'expand'])
  })

  test('still expands when fullscreen request fails', () => {
    const webApp = {
      ready: vi.fn(),
      requestFullscreen: vi.fn(() => {
        throw new Error('fullscreen unavailable')
      }),
      expand: vi.fn(),
    }

    expect(() => initializeTelegramWebApp(webApp)).not.toThrow()
    expect(webApp.expand).toHaveBeenCalledOnce()
  })
})
