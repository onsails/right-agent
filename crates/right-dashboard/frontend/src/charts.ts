import { use } from 'echarts/core'
import { CanvasRenderer } from 'echarts/renderers'
import { BarChart, LineChart, SankeyChart, ThemeRiverChart } from 'echarts/charts'
import {
  DatasetComponent,
  DataZoomComponent,
  GraphicComponent,
  GridComponent,
  LegendComponent,
  SingleAxisComponent,
  TooltipComponent,
} from 'echarts/components'

let registered = false

export function registerDashboardCharts(): void {
  if (registered) {
    return
  }

  use([
    CanvasRenderer,
    BarChart,
    LineChart,
    SankeyChart,
    ThemeRiverChart,
    DatasetComponent,
    DataZoomComponent,
    GraphicComponent,
    GridComponent,
    LegendComponent,
    SingleAxisComponent,
    TooltipComponent,
  ])
  registered = true
}
