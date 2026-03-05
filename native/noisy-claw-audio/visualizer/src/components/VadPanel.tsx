import { useEffect, useRef } from 'react'
import type { VadMeta } from '../lib/protocol'

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
    timestamp: parseFloat(parts[0]) / 1000, // ms → sec
    speechProb: parseFloat(parts[1]),
    isSpeech: parts[2] === '1',
    speakingTts: parts[3] === '1',
    blanking: parseInt(parts[4]),
    wasSpeaking: parts[5] === '1',
  }
}

export function VadPanel({ onVadMeta, height = 140, durationSec = 10 }: VadPanelProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const pointsRef = useRef<VadPoint[]>([])
  const animRef = useRef<number>(0)

  useEffect(() => {
    const unsubscribe = onVadMeta((meta) => {
      const point = parseCsv(meta.data)
      if (!point) return
      // Use the tap timestamp instead of CSV elapsed_ms for alignment
      point.timestamp = meta.timestamp
      pointsRef.current.push(point)

      // Keep last N seconds
      const cutoff = point.timestamp - durationSec
      while (pointsRef.current.length > 0 && pointsRef.current[0].timestamp < cutoff) {
        pointsRef.current.shift()
      }
    })

    return unsubscribe
  }, [onVadMeta, durationSec])

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const ctx = canvas.getContext('2d')
    if (!ctx) return

    const draw = () => {
      const w = canvas.width
      const h = canvas.height
      const points = pointsRef.current

      ctx.fillStyle = '#0a0a0a'
      ctx.fillRect(0, 0, w, h)

      if (points.length === 0) {
        ctx.fillStyle = '#666'
        ctx.font = '11px monospace'
        ctx.fillText('Waiting for VAD data...', 4, 14)
        animRef.current = requestAnimationFrame(draw)
        return
      }

      const now = points[points.length - 1].timestamp
      const start = now - durationSec

      // Draw background regions
      for (const pt of points) {
        const x = ((pt.timestamp - start) / durationSec) * w
        const barW = Math.max(1, w / (durationSec * 30)) // ~30 points/sec

        if (pt.speakingTts) {
          ctx.fillStyle = 'rgba(239, 68, 68, 0.15)' // red tint for TTS
          ctx.fillRect(x, 0, barW, h)
        }
        if (pt.blanking > 0) {
          ctx.fillStyle = 'rgba(234, 179, 8, 0.15)' // yellow tint for blanking
          ctx.fillRect(x, 0, barW, h)
        }
        if (pt.isSpeech) {
          ctx.fillStyle = 'rgba(34, 197, 94, 0.2)' // green tint for speech
          ctx.fillRect(x, h * 0.6, barW, h * 0.4)
        }
      }

      // Draw speech probability line
      ctx.strokeStyle = '#22d3ee' // cyan
      ctx.lineWidth = 1.5
      ctx.beginPath()
      let first = true
      for (const pt of points) {
        const x = ((pt.timestamp - start) / durationSec) * w
        const y = h - pt.speechProb * h
        if (first) {
          ctx.moveTo(x, y)
          first = false
        } else {
          ctx.lineTo(x, y)
        }
      }
      ctx.stroke()

      // Draw threshold line at 0.5
      ctx.strokeStyle = '#666'
      ctx.lineWidth = 1
      ctx.setLineDash([4, 4])
      ctx.beginPath()
      ctx.moveTo(0, h * 0.5)
      ctx.lineTo(w, h * 0.5)
      ctx.stroke()
      ctx.setLineDash([])

      // Labels
      ctx.fillStyle = '#22d3ee'
      ctx.font = '11px monospace'
      ctx.fillText('VAD', 4, 14)

      ctx.fillStyle = '#666'
      ctx.font = '10px monospace'
      ctx.fillText('1.0', w - 24, 12)
      ctx.fillText('0.5', w - 24, h / 2 + 4)
      ctx.fillText('0.0', w - 24, h - 4)

      // Legend
      const legendY = 14
      let legendX = 40
      const items = [
        { color: 'rgba(34, 197, 94, 0.6)', label: 'speech' },
        { color: 'rgba(239, 68, 68, 0.5)', label: 'TTS' },
        { color: 'rgba(234, 179, 8, 0.5)', label: 'blanking' },
      ]
      for (const item of items) {
        ctx.fillStyle = item.color
        ctx.fillRect(legendX, legendY - 8, 8, 8)
        ctx.fillStyle = '#999'
        ctx.fillText(item.label, legendX + 10, legendY)
        legendX += ctx.measureText(item.label).width + 20
      }

      animRef.current = requestAnimationFrame(draw)
    }

    animRef.current = requestAnimationFrame(draw)
    return () => cancelAnimationFrame(animRef.current)
  }, [durationSec])

  return (
    <canvas
      ref={canvasRef}
      width={800}
      height={height}
      className="w-full rounded border border-gray-800"
    />
  )
}
