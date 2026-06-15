export const JEWEL_CHART_PALETTE = ['#3bb0c4', '#c75f88', '#cda14b', '#6bbf59', '#b6a8b0', '#e6c06a']

/**
 * Spreadable root fragment merged into every dashboard ECharts option for
 * jewel-dark legibility. Contains only valid top-level option keys, so
 * `...jewelChartBase` never leaks non-option keys into the chart config.
 */
export const jewelChartBase = {
  backgroundColor: 'transparent',
  color: JEWEL_CHART_PALETTE,
  textStyle: { color: '#b6a8b0' },
  title: { textStyle: { color: '#f1ece9' } },
  legend: { textStyle: { color: '#b6a8b0' } },
  tooltip: {
    backgroundColor: '#201a26',
    borderColor: '#2d2533',
    textStyle: { color: '#f1ece9' },
  },
} as const

/**
 * Axis styling fragment. Merge into an `xAxis`/`yAxis` explicitly — it is NOT
 * a top-level option key, so it must never be spread at the option root.
 */
export const jewelAxis = {
  axisLine: { lineStyle: { color: '#2d2533' } },
  axisLabel: { color: '#b6a8b0' },
  splitLine: { lineStyle: { color: '#2d2533' } },
} as const
