import { describe, expect, it } from 'vitest'

import { dashboardTabItems, dashboardTabs, isDashboardTab, normalizeInitialTab } from './dashboardTabs'

describe('App tab rendering', () => {
  it('recognizes MCP as a valid dashboard tab', () => {
    expect(isDashboardTab('mcp')).toBe(true)
    expect(normalizeInitialTab('mcp')).toBe('mcp')
    expect(normalizeInitialTab('unknown')).toBe('overview')
  })

  it('builds one tab item for every valid dashboard tab', () => {
    expect(dashboardTabItems().map((tab) => tab.key)).toEqual(dashboardTabs)
    expect(dashboardTabItems().find((tab) => tab.key === 'mcp')).toEqual({ key: 'mcp', label: 'MCP', enabled: true })
  })

  it('keeps feature-gated tab enablement separate from MCP', () => {
    const tabs = dashboardTabItems({
      activity: false,
      knowledge_learning: false,
      knowledge_skills: false,
      usage: false,
      identity: false,
      doctor: false,
      sandbox_stats: false,
    })

    expect(tabs.find((tab) => tab.key === 'activity')?.enabled).toBe(false)
    expect(tabs.find((tab) => tab.key === 'knowledge')?.enabled).toBe(false)
    expect(tabs.find((tab) => tab.key === 'mcp')?.enabled).toBe(true)
  })
})
