import { useEffect, useRef } from 'react'
import type { AudioFrame } from '../lib/protocol'
import { useECharts } from '../hooks/useECharts'
import { fftMagnitude } from '../lib/fft'
import { useTheme, getTokens } from '../lib/theme'

interface SpectrogramCanvasProps {
  tap: string
  color: string
  onFrame: (listener: (tap: string, frame: AudioFrame) => void) => () => void
  height?: number
  sampleRate?: number
}

const FFT_SIZE = 2048
const FREQ_BINS = 80
const TIME_COLS = 200

// Dark: inferno-like (black → purple → orange → yellow)
const COLORMAP_DARK = [
  '#000004', '#06051a', '#140b34', '#230b52',
  '#3b0964', '#51127c', '#65156e', '#7e2482',
  '#952c80', '#b73779', '#cf4446', '#e36130',
  '#ed7953', '#f89540', '#fbb61a', '#f5d745',
  '#fcffa4',
]

// Light: white → blue → teal → green → yellow
const COLORMAP_LIGHT = [
  '#FAFAFA', '#EEF2FF', '#DBEAFE', '#BAE6FD',
  '#7DD3FC', '#38BDF8', '#0EA5E9', '#0284C7',
  '#0369A1', '#0C4A6E', '#155E75', '#115E59',
  '#047857', '#15803D', '#65A30D', '#CA8A04',
  '#B45309',
]

export function SpectrogramCanvas({
  tap,
  color,
  onFrame,
  height = 200,
  sampleRate = 48000,
}: SpectrogramCanvasProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const containerRef = useRef<HTMLDivElement>(null)
  const bufferRef = useRef<Float32Array>(new Float32Array(FFT_SIZE))
  const writeOffsetRef = useRef(0)
  const chart = useECharts(containerRef)

  // 2D spectrogram grid: gridRef[y] is a row of TIME_COLS values
  const gridRef = useRef<Float32Array[] | null>(null)
  if (!gridRef.current) {
    gridRef.current = Array.from({ length: FREQ_BINS }, () => new Float32Array(TIME_COLS))
  }

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
    const nyquist = sampleRate / 2
    const binWidth = nyquist / FREQ_BINS

    const freqLabels = Array.from({ length: FREQ_BINS }, (_, i) => {
      const freq = (i + 0.5) * binWidth
      return freq >= 1000 ? `${(freq / 1000).toFixed(1)} kHz` : `${Math.round(freq)} Hz`
    })

    const targetFreqs = [440, 1000, 2000, 5000, 10000, 15000, 20000]

    chart.current?.setOption({
      backgroundColor: 'transparent',
      title: {
        text: `${tap} spectrogram`,
        left: 4,
        top: 2,
        textStyle: { color, fontSize: 11, fontFamily: 'monospace', fontWeight: 'normal' },
      },
      grid: { left: 10, right: 65, top: 24, bottom: 10 },
      xAxis: {
        type: 'category',
        data: Array.from({ length: TIME_COLS }, (_, i) => i),
        show: false,
      },
      yAxis: {
        type: 'category',
        data: freqLabels,
        position: 'right',
        axisLabel: {
          interval: (index: number) => {
            const freq = (index + 0.5) * binWidth
            return targetFreqs.some(t => Math.abs(freq - t) < binWidth * 0.8)
          },
          color: tk.textMuted,
          fontSize: 9,
          fontFamily: 'monospace',
        },
        axisTick: { show: false },
        axisLine: { show: false },
        splitLine: { show: false },
      },
      visualMap: {
        min: 0,
        max: 1,
        show: false,
        inRange: { color: theme === 'dark' ? COLORMAP_DARK : COLORMAP_LIGHT },
      },
      series: [{
        type: 'heatmap',
        data: [],
        itemStyle: { borderWidth: 0 },
        emphasis: { disabled: true },
        progressive: 0,
        animation: false,
      }],
    })
  }, [chart, tap, color, sampleRate, tk, theme])

  // Chart update via requestAnimationFrame (~15fps)
  useEffect(() => {
    const numBins = FFT_SIZE >> 1
    const binsPerBar = Math.floor(numBins / FREQ_BINS)
    let raf = 0
    let lastTime = 0

    const update = (time: number) => {
      raf = requestAnimationFrame(update)
      if (time - lastTime < 67) return // ~15fps
      lastTime = time
      const c = chart.current
      if (!c) return
      const grid = gridRef.current
      if (!grid) return

      // Read last FFT_SIZE samples from the ring buffer
      const buf = bufferRef.current
      const offset = writeOffsetRef.current
      const input = new Float32Array(FFT_SIZE)
      for (let i = 0; i < FFT_SIZE; i++) {
        input[i] = buf[(offset + i) % buf.length]
      }

      const mags = fftMagnitude(input, FFT_SIZE)

      // Shift grid left and insert new column
      for (let y = 0; y < FREQ_BINS; y++) {
        const row = grid[y]
        row.copyWithin(0, 1)
        const start = y * binsPerBar
        const end = Math.min(start + binsPerBar, numBins)
        let max = 0
        for (let i = start; i < end; i++) {
          if (mags[i] > max) max = mags[i]
        }
        row[TIME_COLS - 1] = max
      }

      // Build heatmap data — skip near-zero for performance
      const data: number[][] = []
      for (let x = 0; x < TIME_COLS; x++) {
        for (let y = 0; y < FREQ_BINS; y++) {
          const v = grid[y][x]
          if (v > 0.01) {
            data.push([x, y, v])
          }
        }
      }

      c.setOption({ series: [{ data }] })
    }

    raf = requestAnimationFrame(update)
    return () => cancelAnimationFrame(raf)
  }, [chart, sampleRate])

  return (
    <div
      ref={containerRef}
      style={{ height, border: `1px solid ${tk.borderPrimary}`, backgroundColor: theme === 'dark' ? '#000004' : '#FAFAFA' }}
      className="w-full rounded"
    />
  )
}
