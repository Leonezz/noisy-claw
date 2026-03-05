import { useEffect, useRef } from 'react'
import type { VadMeta } from '../lib/protocol'
import { useECharts } from '../hooks/useECharts'

interface VadPanelProps {
  onVadMeta: (listener: (meta: VadMeta) => void) => () => void
  height?: number
  durationSec?: number
}

interface VadPoint {
  timestamp: number
  speechProb: number
  isSpeech: boolean
  speakingTts: boolean
  blanking: number
  wasSpeaking: boolean
}

function parseCsv(data: string): VadPoint | null {
  const parts = data.trim().split(',')
  if (parts.length < 6) return null
  return {
    timestamp: parseFloat(parts[0]) / 1000, // ms -> sec
    speechProb: parseFloat(parts[1]),
    isSpeech: parts[2] === '1',
    speakingTts: parts[3] === '1',
    blanking: parseInt(parts[4]),
    wasSpeaking: parts[5] === '1',
  }
}

function computeRanges(
  points: VadPoint[],
  predicate: (p: VadPoint) => boolean,
): [number, number][] {
  const ranges: [number, number][] = []
  let start: number | null = null
  for (const p of points) {
    if (predicate(p)) {
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

export function VadPanel({ onVadMeta, height = 140, durationSec = 10 }: VadPanelProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const pointsRef = useRef<VadPoint[]>([])
  const chart = useECharts(containerRef)

  // Subscribe to VAD metadata
  useEffect(() => {
    return onVadMeta((meta) => {
      const point = parseCsv(meta.data)
      if (!point) return
      point.timestamp = meta.timestamp
      pointsRef.current.push(point)
      const cutoff = point.timestamp - durationSec
      while (pointsRef.current.length > 0 && pointsRef.current[0].timestamp < cutoff) {
        pointsRef.current.shift()
      }
    })
  }, [onVadMeta, durationSec])

  // Initial chart config
  useEffect(() => {
    chart.current?.setOption({
      backgroundColor: 'transparent',
      title: {
        text: 'VAD',
        left: 4,
        top: 2,
        textStyle: { color: '#22d3ee', fontSize: 11, fontFamily: 'monospace', fontWeight: 'normal' },
      },
      tooltip: {
        trigger: 'axis',
        backgroundColor: 'rgba(0,0,0,0.8)',
        borderColor: '#333',
        textStyle: { color: '#ccc', fontSize: 11 },
        formatter: (params: any) => {
          const p = Array.isArray(params) ? params[0] : params
          if (!p?.value) return ''
          return `Prob: ${(p.value[1] * 100).toFixed(1)}%`
        },
      },
      grid: { left: 35, right: 30, top: 20, bottom: 20 },
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
          color: '#666',
          fontSize: 10,
          fontFamily: 'monospace',
        },
        splitLine: { lineStyle: { color: '#1e293b' } },
      },
      legend: {
        right: 10,
        top: 0,
        textStyle: { color: '#999', fontSize: 10 },
        itemWidth: 12,
        itemHeight: 8,
      },
      series: [
        {
          name: 'speech prob',
          type: 'line',
          data: [],
          smooth: false,
          symbol: 'none',
          lineStyle: { color: '#22d3ee', width: 1.5 },
          itemStyle: { color: '#22d3ee' },
          markLine: {
            silent: true,
            symbol: 'none',
            lineStyle: { color: '#666', type: 'dashed' },
            data: [{ yAxis: 0.5 }],
            label: { show: false },
          },
          markArea: { silent: true, data: [] },
          animation: false,
        },
        // Dummy series for legend colors
        { name: 'speech', type: 'line', data: [], symbol: 'none', lineStyle: { color: 'rgba(34,197,94,0.6)', width: 4 }, itemStyle: { color: 'rgba(34,197,94,0.6)' } },
        { name: 'TTS', type: 'line', data: [], symbol: 'none', lineStyle: { color: 'rgba(239,68,68,0.5)', width: 4 }, itemStyle: { color: 'rgba(239,68,68,0.5)' } },
        { name: 'blanking', type: 'line', data: [], symbol: 'none', lineStyle: { color: 'rgba(234,179,8,0.5)', width: 4 }, itemStyle: { color: 'rgba(234,179,8,0.5)' } },
      ],
    })
  }, [chart])

  // Throttled chart update at ~10 fps
  useEffect(() => {
    const interval = setInterval(() => {
      const c = chart.current
      if (!c) return
      const points = pointsRef.current
      if (points.length === 0) return

      const now = points[points.length - 1].timestamp
      const start = now - durationSec

      const probData = points.map((p) => [p.timestamp, p.speechProb])

      const markAreaData = [
        ...computeRanges(points, (p) => p.isSpeech).map(([s, e]) => [
          { xAxis: s, itemStyle: { color: 'rgba(34,197,94,0.2)' } },
          { xAxis: e },
        ]),
        ...computeRanges(points, (p) => p.speakingTts).map(([s, e]) => [
          { xAxis: s, itemStyle: { color: 'rgba(239,68,68,0.15)' } },
          { xAxis: e },
        ]),
        ...computeRanges(points, (p) => p.blanking > 0).map(([s, e]) => [
          { xAxis: s, itemStyle: { color: 'rgba(234,179,8,0.15)' } },
          { xAxis: e },
        ]),
      ]

      c.setOption({
        xAxis: { min: start, max: now },
        series: [{ data: probData, markArea: { data: markAreaData } }],
      })
    }, 100)

    return () => clearInterval(interval)
  }, [durationSec, chart])

  return (
    <div
      ref={containerRef}
      style={{ height }}
      className="w-full rounded border border-gray-800"
    />
  )
}
