import { describe, expect, it } from 'vitest'

import app from './App.vue?raw'

describe('App tab rendering', () => {
  it('has an explicit render branch for every valid dashboard tab', () => {
    const validTabs = app.match(/return \[([\s\S]*?)\]\.includes\(tab\)/)?.[1] ?? ''
    const renderedTabs = Array.from(app.matchAll(/activeTab === '([^']+)'/g), (match: RegExpMatchArray) => match[1])

    expect(validTabs).toContain("'mcp'")
    expect(new Set(renderedTabs)).toEqual(new Set(['overview', 'activity', 'knowledge', 'usage', 'identity', 'health', 'mcp']))
  })
})
