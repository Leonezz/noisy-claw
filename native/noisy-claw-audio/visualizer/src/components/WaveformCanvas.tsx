import { useEffect, useRef } from 'react'
import type { AudioFrame } from '../lib/protocol'
import { useECharts } from '../hooks/useECharts'
import { envelope } from '../lib/downsample'

interface WaveformCanvasProps {
  tap: string
  color: string
  onFrame: (listener: (tap: string, frame: AudioFrame) => void) => () => void
  height?: number
  durationSec?: number
  sampleRate?: number
}

const NUM_BUCKETS = 600

export function WaveformCanvas({
  tap,
  color,
  onFrame,
  height = 120,
  durationSec = 10,
  sampleRate = 48000,
}: WaveformCanvasProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const bufferRef = useRef<Float32Array>(new Float32Array(sampleRate * durationSec))
  const writeOffsetRef = useRef(0)
  const chart = useECharts(containerRef)

  useEffect(() => {
    bufferRef.current = new Float32Array(sampleRate * durationSec)
    writeOffsetRef.current = 0
  }, [sampleRate, durationSec])

  // Subscribe to audio frames — write into ring buffer
  useEffect(() => {
    return onFrame((frameTap, frame) => {
      if (frameTap !== tap) return
      const buf = bufferRef.current
      for (let i = 0; i < frame.samples.length; i++) {
        buf[writeOffsetRef.current] = frame.samples[i]
        writeOffsetRef.current = (writeOffsetRef.current + 1) % buf.length
      }
    })
  }, [tap, onFrame])

  // Initial chart config
  useEffect(() => {
    chart.current?.setOption({
      backgroundColor: 'transparent',
      title: {
        text: tap,
        left: 4,
        top: 2,
        textStyle: { color, fontSize: 11, fontFamily: 'monospace', fontWeight: 'normal' },
      },
      grid: { left: 0, right: 0, top: 0, bottom: 0 },
      xAxis: {
        type: 'value',
        show: false,
        min: 0,
        max: NUM_BUCKETS - 1,
      },
      yAxis: {
        type: 'value',
        show: false,
        min: -1,
        max: 1,
      },
      series: [
        {
          type: 'custom',
          renderItem: (_params: unknown, api: any) => {
            const x = api.coord([api.value(0), 0])[0]
            const yMax = api.coord([0, api.value(1)])[1]
            const yMin = api.coord([0, api.value(2)])[1]
            return {
              type: 'line',
              shape: { x1: x, y1: yMax, x2: x, y2: yMin },
              style: { stroke: color, lineWidth: 1.5 },
            }
          },
          data: [],
          animation: false,
          silent: true,
        },
      ],
    })
  }, [tap, color, chart])

  // Throttled chart update at ~15 fps
  useEffect(() => {
    const interval = setInterval(() => {
      const c = chart.current
      if (!c) return
      const env = envelope(bufferRef.current, writeOffsetRef.current, NUM_BUCKETS)
      const data = env.map(([max, min], i) => [i, max, min])
      c.setOption({ series: [{ data }] })
    }, 66)

    return () => clearInterval(interval)
  }, [chart])

  return (
    <div
      ref={containerRef}
      style={{ height }}
      className="w-full rounded border border-gray-800"
    />
  )
}
