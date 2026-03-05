import { useEffect, useRef } from 'react'
import * as echarts from 'echarts'

export function useECharts(
  containerRef: React.RefObject<HTMLDivElement | null>,
): React.RefObject<echarts.ECharts | null> {
  const chartRef = useRef<echarts.ECharts | null>(null)

  useEffect(() => {
    const el = containerRef.current
    if (!el) return

    const chart = echarts.init(el, 'dark', { renderer: 'canvas' })
    chartRef.current = chart

    const ro = new ResizeObserver(() => chart.resize())
    ro.observe(el)

    return () => {
      ro.disconnect()
      chart.dispose()
      chartRef.current = null
    }
  }, []) // containerRef is a stable ref object

  return chartRef as React.RefObject<echarts.ECharts | null>
}
