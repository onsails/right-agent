import type { DashboardFeatures } from './types'

export const dashboardTabs = ['overview', 'activity', 'knowledge', 'usage', 'identity', 'health', 'mcp', 'providers'] as const

export type DashboardTab = typeof dashboardTabs[number]

export interface DashboardTabItem {
  key: DashboardTab
  label: string
  enabled: boolean
}

export function dashboardTabItems(features?: Partial<DashboardFeatures>): DashboardTabItem[] {
  return [
    { key: 'overview', label: 'Overview', enabled: true },
    { key: 'activity', label: 'Activity', enabled: features?.activity ?? true },
    { key: 'knowledge', label: 'Knowledge', enabled: (features?.knowledge_learning ?? true) || (features?.knowledge_skills ?? true) },
    { key: 'usage', label: 'Usage', enabled: features?.usage ?? true },
    { key: 'identity', label: 'Identity', enabled: features?.identity ?? true },
    { key: 'health', label: 'Health', enabled: (features?.doctor ?? true) || (features?.sandbox_stats ?? true) },
    { key: 'mcp', label: 'MCP', enabled: true },
    { key: 'providers', label: 'Providers', enabled: true },
  ]
}

export function normalizeInitialTab(tab: string): DashboardTab {
  return isDashboardTab(tab) ? tab : 'overview'
}

export function isDashboardTab(tab: string): tab is DashboardTab {
  return dashboardTabs.includes(tab as DashboardTab)
}
