export const JEWEL_CHART_PALETTE = ['#3bb0c4', '#c75f88', '#cda14b', '#6bbf59', '#b6a8b0', '#e6c06a']

/** Option fragment merged into every dashboard ECharts option for jewel-dark legibility. */
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
  categoryAxis: {
    axisLine: { lineStyle: { color: '#2d2533' } },
    axisLabel: { color: '#b6a8b0' },
    splitLine: { lineStyle: { color: '#2d2533' } },
  },
  valueAxis: {
    axisLine: { lineStyle: { color: '#2d2533' } },
    axisLabel: { color: '#b6a8b0' },
    splitLine: { lineStyle: { color: '#2d2533' } },
  },
} as const
