import { useEffect, useRef } from 'react'
import type { AudioFrame } from '../lib/protocol'

interface WaveformCanvasProps {
  tap: string
  color: string
  onFrame: (listener: (tap: string, frame: AudioFrame) => void) => () => void
  height?: number
  durationSec?: number
  sampleRate?: number
}

export function WaveformCanvas({
  tap,
  color,
  onFrame,
  height = 120,
  durationSec = 10,
  sampleRate = 48000,
}: WaveformCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const bufferRef = useRef<Float32Array>(new Float32Array(sampleRate * durationSec))
  const writeOffsetRef = useRef(0)
  const animRef = useRef<number>(0)

  useEffect(() => {
    // Reallocate buffer if params change
    bufferRef.current = new Float32Array(sampleRate * durationSec)
    writeOffsetRef.current = 0
  }, [sampleRate, durationSec])

  useEffect(() => {
    const unsubscribe = onFrame((frameTap, frame) => {
      if (frameTap !== tap) return

      const buf = bufferRef.current
      const samples = frame.samples

      // Ring-buffer write
      for (let i = 0; i < samples.length; i++) {
        buf[writeOffsetRef.current] = samples[i]
        writeOffsetRef.current = (writeOffsetRef.current + 1) % buf.length
      }
    })

    return unsubscribe
  }, [tap, onFrame])

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return

    const ctx = canvas.getContext('2d')
    if (!ctx) return

    const draw = () => {
      const w = canvas.width
      const h = canvas.height
      const buf = bufferRef.current
      const totalSamples = buf.length
      const offset = writeOffsetRef.current

      ctx.fillStyle = '#0a0a0a'
      ctx.fillRect(0, 0, w, h)

      // Draw center line
      ctx.strokeStyle = '#333'
      ctx.lineWidth = 1
      ctx.beginPath()
      ctx.moveTo(0, h / 2)
      ctx.lineTo(w, h / 2)
      ctx.stroke()

      // Draw waveform (min/max per pixel column)
      ctx.strokeStyle = color
      ctx.lineWidth = 1
      ctx.beginPath()

      const samplesPerPixel = totalSamples / w

      for (let px = 0; px < w; px++) {
        const startSample = Math.floor(px * samplesPerPixel)
        const endSample = Math.floor((px + 1) * samplesPerPixel)

        let min = 0
        let max = 0
        for (let i = startSample; i < endSample; i++) {
          const idx = (offset + i) % totalSamples
          const s = buf[idx]
          if (s < min) min = s
          if (s > max) max = s
        }

        const yMin = h / 2 - max * (h / 2)
        const yMax = h / 2 - min * (h / 2)

        ctx.moveTo(px, yMin)
        ctx.lineTo(px, yMax)
      }

      ctx.stroke()

      // Label
      ctx.fillStyle = color
      ctx.font = '11px monospace'
      ctx.fillText(tap, 4, 14)

      animRef.current = requestAnimationFrame(draw)
    }

    animRef.current = requestAnimationFrame(draw)
    return () => cancelAnimationFrame(animRef.current)
  }, [tap, color])

  return (
    <canvas
      ref={canvasRef}
      width={800}
      height={height}
      className="w-full rounded border border-gray-800"
    />
  )
}
