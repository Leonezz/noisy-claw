import { useEffect, useRef } from 'react'
import type { AudioFrame } from '../lib/protocol'
import { useECharts } from '../hooks/useECharts'
import { fftMagnitude } from '../lib/fft'
import { useTheme, getTokens } from '../lib/theme'

interface SpectrumCanvasProps {
  tap: string
  color: string
  onFrame: (listener: (tap: string, frame: AudioFrame) => void) => () => void
  height?: number
  sampleRate?: number
}

const FFT_SIZE = 2048

export function SpectrumCanvas({
  tap,
  color,
  onFrame,
  height = 80,
  sampleRate = 48000,
}: SpectrumCanvasProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const containerRef = useRef<HTMLDivElement>(null)
  const bufferRef = useRef<Float32Array>(new Float32Array(FFT_SIZE))
  const writeOffsetRef = useRef(0)
  const chart = useECharts(containerRef)

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
    chart.current?.setOption({
      backgroundColor: 'transparent',
      title: {
        text: `${tap} spectrum`,
        left: 4,
        top: 2,
        textStyle: { color, fontSize: 11, fontFamily: 'monospace', fontWeight: 'normal' },
      },
      grid: { left: 35, right: 10, top: 24, bottom: 20 },
      xAxis: {
        type: 'value',
        min: 0,
        max: nyquist,
        axisLabel: {
          formatter: (v: number) => v >= 1000 ? `${(v / 1000).toFixed(0)}k` : `${v}`,
          color: tk.textMuted,
          fontSize: 9,
          fontFamily: 'monospace',
        },
        splitLine: { show: false },
        axisLine: { lineStyle: { color: tk.borderPrimary } },
        axisTick: { lineStyle: { color: tk.borderPrimary } },
      },
      yAxis: {
        type: 'value',
        min: 0,
        max: 1,
        show: false,
      },
      series: [
        {
          type: 'bar',
          data: [],
          barWidth: '100%',
          barGap: '0%',
          itemStyle: { color },
          animation: false,
          silent: true,
          large: true,
        },
      ],
    })
  }, [tap, color, chart, sampleRate, tk])

  // Chart update via requestAnimationFrame (~20fps)
  useEffect(() => {
    const nyquist = sampleRate / 2
    const numBins = FFT_SIZE >> 1
    let raf = 0
    let lastTime = 0
    const update = (time: number) => {
      raf = requestAnimationFrame(update)
      if (time - lastTime < 50) return
      lastTime = time
      const c = chart.current
      if (!c) return

      // Read last FFT_SIZE samples from the ring buffer
      const buf = bufferRef.current
      const offset = writeOffsetRef.current
      const input = new Float32Array(FFT_SIZE)
      for (let i = 0; i < FFT_SIZE; i++) {
        input[i] = buf[(offset + i) % buf.length]
      }

      const mags = fftMagnitude(input, FFT_SIZE)

      // Downsample to ~128 bars for display
      const barCount = 128
      const binsPerBar = Math.floor(numBins / barCount)
      const data: [number, number][] = []
      for (let b = 0; b < barCount; b++) {
        const start = b * binsPerBar
        const end = start + binsPerBar
        let max = 0
        for (let i = start; i < end && i < numBins; i++) {
          if (mags[i] > max) max = mags[i]
        }
        const freq = ((b + 0.5) * binsPerBar / numBins) * nyquist
        data.push([freq, max])
      }

      c.setOption({ series: [{ data }] })
    }
    raf = requestAnimationFrame(update)
    return () => cancelAnimationFrame(raf)
  }, [chart, sampleRate])

  return (
    <div
      ref={containerRef}
      style={{ height, border: `1px solid ${tk.borderPrimary}` }}
      className="w-full rounded"
    />
  )
}
