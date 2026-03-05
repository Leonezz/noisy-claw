import { useEffect, useRef, useState } from 'react'
import type { AudioFrame } from '../lib/protocol'
import { rmsToDb } from '../lib/protocol'

interface LevelMeterProps {
  tap: string
  color: string
  onFrame: (listener: (tap: string, frame: AudioFrame) => void) => () => void
}

export function LevelMeter({ tap, color, onFrame }: LevelMeterProps) {
  const [db, setDb] = useState(-100)
  const rmsRef = useRef(0)

  useEffect(() => {
    const unsubscribe = onFrame((frameTap, frame) => {
      if (frameTap !== tap) return
      let sum = 0
      for (let i = 0; i < frame.samples.length; i++) {
        sum += frame.samples[i] * frame.samples[i]
      }
      rmsRef.current = Math.sqrt(sum / frame.samples.length)
    })

    const interval = setInterval(() => {
      setDb(rmsToDb(rmsRef.current))
    }, 50)

    return () => {
      unsubscribe()
      clearInterval(interval)
    }
  }, [tap, onFrame])

  // Map dB to percentage (0-100), range -60..0 dB
  const pct = Math.max(0, Math.min(100, ((db + 60) / 60) * 100))

  return (
    <div className="flex items-center gap-2 text-xs font-mono">
      <span className="w-20 truncate" style={{ color }}>
        {tap}
      </span>
      <div className="flex-1 h-3 bg-gray-800 rounded overflow-hidden">
        <div
          className="h-full rounded transition-all duration-75"
          style={{
            width: `${pct}%`,
            backgroundColor: color,
            opacity: 0.8,
          }}
        />
      </div>
      <span className="w-14 text-right text-gray-400">
        {db > -100 ? `${db.toFixed(1)}dB` : '-∞'}
      </span>
    </div>
  )
}
