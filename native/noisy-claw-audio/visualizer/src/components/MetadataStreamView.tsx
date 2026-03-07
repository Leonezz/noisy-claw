import { useEffect, useRef } from 'react'
import type { MetadataEvent, FieldDescriptor } from '../lib/protocol'
import { useECharts } from '../hooks/useECharts'
import { useTheme, getTokens } from '../lib/theme'
import { getTapColor } from '../lib/colors'

interface MetadataStreamViewProps {
  streamName: string
  fields: FieldDescriptor[]
  onMetadata: (listener: (meta: MetadataEvent) => void) => () => void
  height?: number
  durationSec?: number
}

interface DataPoint {
  timestamp: number
  values: Record<string, unknown>
}

function computeRanges(
  points: DataPoint[],
  field: string,
): [number, number][] {
  const ranges: [number, number][] = []
  let start: number | null = null
  for (const p of points) {
    if (p.values[field] === true) {
      if (start === null) start = p.timestamp
    } else if (start !== null) {
      ranges.push([start, p.timestamp])
      start = null
    }
  }
  if (start !== null && points.length > 0) {
    ranges.push([start, points[points.length - 1].timestamp])
  }
  return ranges
}

export function MetadataStreamView({
  streamName,
  fields,
  onMetadata,
  height = 120,
  durationSec = 10,
}: MetadataStreamViewProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const containerRef = useRef<HTMLDivElement>(null)
  const pointsRef = useRef<DataPoint[]>([])
  const chart = useECharts(containerRef)

  const numericFields = fields.filter((f) => f.field_type === 'f64' || f.field_type === 'u32')
  const boolFields = fields.filter((f) => f.field_type === 'bool')

  // Subscribe to metadata stream
  useEffect(() => {
    return onMetadata((meta) => {
      if (meta.stream !== streamName) return
      pointsRef.current.push({ timestamp: meta.timestamp, values: meta.fields })
      const cutoff = meta.timestamp - durationSec
      while (pointsRef.current.length > 0 && pointsRef.current[0].timestamp < cutoff) {
        pointsRef.current.shift()
      }
    })
  }, [onMetadata, streamName, durationSec])

  // Initial chart config
  useEffect(() => {
    const numericSeries = numericFields.map((f, i) => ({
      name: f.name,
      type: 'line' as const,
      data: [] as number[][],
      smooth: false,
      symbol: 'none',
      lineStyle: { color: getTapColor(`${streamName}_${f.name}`), width: 1.5 },
      itemStyle: { color: getTapColor(`${streamName}_${f.name}`) },
      ...(i === 0 ? {
        markLine: {
          silent: true,
          symbol: 'none',
          lineStyle: { color: tk.textMuted, type: 'dashed' as const },
          data: [{ yAxis: 0.5 }],
          label: { show: false },
        },
        markArea: { silent: true, data: [] as unknown[] },
      } : {}),
      animation: false,
    }))

    // Dummy series for bool field legend colors
    const boolLegendSeries = boolFields.map((f, i) => {
      const colors = ['rgba(34,197,94,0.6)', 'rgba(239,68,68,0.5)', 'rgba(234,179,8,0.5)', 'rgba(99,102,241,0.5)']
      const color = colors[i % colors.length]
      return {
        name: f.name,
        type: 'line' as const,
        data: [] as number[][],
        symbol: 'none',
        lineStyle: { color, width: 4 },
        itemStyle: { color },
      }
    })

    chart.current?.setOption({
      backgroundColor: 'transparent',
      title: {
        text: streamName,
        left: 4,
        top: 2,
        textStyle: { color: getTapColor(streamName), fontSize: 11, fontFamily: 'monospace', fontWeight: 'normal' },
      },
      tooltip: {
        trigger: 'axis',
        backgroundColor: theme === 'dark' ? 'rgba(0,0,0,0.8)' : 'rgba(255,255,255,0.9)',
        borderColor: tk.borderSecondary,
        textStyle: { color: tk.textSecondary, fontSize: 11 },
      },
      grid: { left: 35, right: 30, top: 30, bottom: 20 },
      xAxis: {
        type: 'value',
        axisLabel: { show: false },
        splitLine: { show: false },
        axisLine: { show: false },
        axisTick: { show: false },
      },
      yAxis: {
        type: 'value',
        min: 0,
        max: 1,
        splitNumber: 2,
        axisLabel: {
          formatter: (v: number) => v.toFixed(1),
          color: tk.textMuted,
          fontSize: 10,
          fontFamily: 'monospace',
        },
        splitLine: { lineStyle: { color: tk.borderPrimary } },
      },
      legend: {
        right: 10,
        top: 0,
        textStyle: { color: tk.textTertiary, fontSize: 10 },
        itemWidth: 12,
        itemHeight: 8,
      },
      series: [...numericSeries, ...boolLegendSeries],
    })
  }, [chart, theme, tk, streamName, numericFields.length, boolFields.length])

  // Chart update via requestAnimationFrame (~60fps, throttled to ~10fps)
  useEffect(() => {
    let raf = 0
    let lastTime = 0
    const update = (time: number) => {
      raf = requestAnimationFrame(update)
      if (time - lastTime < 100) return // ~10fps throttle
      lastTime = time
      const c = chart.current
      if (!c) return
      const points = pointsRef.current
      if (points.length === 0) return

      const now = points[points.length - 1].timestamp
      const start = now - durationSec

      // Build series data for each numeric field
      const seriesUpdates: Record<string, unknown>[] = numericFields.map((f, i) => {
        const data = points.map((p) => [p.timestamp, typeof p.values[f.name] === 'number' ? p.values[f.name] : 0])

        if (i === 0) {
          // First numeric series gets bool mark areas
          const boolColors = ['rgba(34,197,94,0.2)', 'rgba(239,68,68,0.15)', 'rgba(234,179,8,0.15)', 'rgba(99,102,241,0.15)']
          const markAreaData = boolFields.flatMap((bf, bi) =>
            computeRanges(points, bf.name).map(([s, e]) => [
              { xAxis: s, itemStyle: { color: boolColors[bi % boolColors.length] } },
              { xAxis: e },
            ]),
          )
          return { data, markArea: { data: markAreaData } }
        }
        return { data }
      })

      c.setOption({
        xAxis: { min: start, max: now },
        series: seriesUpdates,
      })
    }
    raf = requestAnimationFrame(update)
    return () => cancelAnimationFrame(raf)
  }, [durationSec, chart, numericFields.length, boolFields.length])

  return (
    <div
      ref={containerRef}
      style={{ height, border: `1px solid ${tk.borderPrimary}` }}
      className="w-full rounded"
    />
  )
}
