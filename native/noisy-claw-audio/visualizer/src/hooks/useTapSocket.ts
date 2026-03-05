import { useCallback, useEffect, useRef, useState } from 'react'
import { type AudioFrame, type VadMeta, type DumpEntry, parseAudioFrame } from '../lib/protocol'

export interface TapState {
  connected: boolean
  frames: Map<string, AudioFrame[]>
  vadMeta: VadMeta[]
  levels: Map<string, number>
  availableTaps: Set<string>
}

interface UseTapSocketOptions {
  url: string
  subscriptions: string[]
  maxFramesPerTap?: number
  paused?: boolean
}

export function useTapSocket({
  url,
  subscriptions,
  maxFramesPerTap = 300,
  paused = false,
}: UseTapSocketOptions) {
  const [connected, setConnected] = useState(false)
  const [availableTaps, setAvailableTaps] = useState<Set<string>>(new Set())
  const wsRef = useRef<WebSocket | null>(null)
  const framesRef = useRef<Map<string, AudioFrame[]>>(new Map())
  const vadMetaRef = useRef<VadMeta[]>([])
  const levelsRef = useRef<Map<string, number>>(new Map())
  const listenersRef = useRef<Set<(tap: string, frame: AudioFrame) => void>>(new Set())
  const vadListenersRef = useRef<Set<(meta: VadMeta) => void>>(new Set())
  const pausedRef = useRef(paused)
  pausedRef.current = paused

  useEffect(() => {
    const ws = new WebSocket(url)
    wsRef.current = ws

    ws.binaryType = 'arraybuffer'

    ws.onopen = () => {
      setConnected(true)
      // Subscribe to selected taps
      const sub = subscriptions.includes('*') ? '*' : subscriptions
      ws.send(JSON.stringify({ subscribe: sub }))
    }

    ws.onmessage = (event) => {
      if (pausedRef.current) return

      if (event.data instanceof ArrayBuffer) {
        try {
          const frame = parseAudioFrame(event.data)

          // Track available taps
          setAvailableTaps((prev) => {
            if (prev.has(frame.tap)) return prev
            return new Set([...prev, frame.tap])
          })

          // Store frame
          const tapFrames = framesRef.current.get(frame.tap) ?? []
          tapFrames.push(frame)
          if (tapFrames.length > maxFramesPerTap) {
            tapFrames.splice(0, tapFrames.length - maxFramesPerTap)
          }
          framesRef.current.set(frame.tap, tapFrames)

          // Compute level
          let sum = 0
          for (let i = 0; i < frame.samples.length; i++) {
            sum += frame.samples[i] * frame.samples[i]
          }
          levelsRef.current.set(frame.tap, Math.sqrt(sum / frame.samples.length))

          // Notify listeners
          for (const listener of listenersRef.current) {
            listener(frame.tap, frame)
          }
        } catch (e) {
          console.error('Failed to parse audio frame:', e, 'byteLength:', event.data.byteLength)
        }
      } else if (typeof event.data === 'string') {
        try {
          const msg = JSON.parse(event.data)
          if (msg.type === 'vad_meta') {
            const meta: VadMeta = msg
            vadMetaRef.current.push(meta)
            if (vadMetaRef.current.length > 1000) {
              vadMetaRef.current.splice(0, vadMetaRef.current.length - 1000)
            }
            for (const listener of vadListenersRef.current) {
              listener(meta)
            }
          }
        } catch {
          // ignore non-JSON text
        }
      }
    }

    ws.onclose = () => setConnected(false)
    ws.onerror = () => setConnected(false)

    return () => {
      ws.close()
      wsRef.current = null
    }
  }, [url, maxFramesPerTap, subscriptions])

  const onFrame = useCallback(
    (listener: (tap: string, frame: AudioFrame) => void) => {
      listenersRef.current.add(listener)
      return () => {
        listenersRef.current.delete(listener)
      }
    },
    [],
  )

  const onVadMeta = useCallback(
    (listener: (meta: VadMeta) => void) => {
      vadListenersRef.current.add(listener)
      return () => {
        vadListenersRef.current.delete(listener)
      }
    },
    [],
  )

  const sendCommand = useCallback((cmd: Record<string, unknown>) => {
    const ws = wsRef.current
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(cmd))
    }
  }, [])

  const listDumps = useCallback((): Promise<DumpEntry[]> => {
    return new Promise((resolve) => {
      const ws = wsRef.current
      if (!ws || ws.readyState !== WebSocket.OPEN) {
        resolve([])
        return
      }

      const handler = (event: MessageEvent) => {
        if (typeof event.data === 'string') {
          try {
            const msg = JSON.parse(event.data)
            if (msg.dumps) {
              ws.removeEventListener('message', handler)
              resolve(msg.dumps as DumpEntry[])
            }
          } catch { /* ignore */ }
        }
      }
      ws.addEventListener('message', handler)
      ws.send(JSON.stringify({ list_dumps: true }))

      // Timeout
      setTimeout(() => {
        ws.removeEventListener('message', handler)
        resolve([])
      }, 5000)
    })
  }, [])

  const requestDumpFile = useCallback(
    (path: string, tap: string): Promise<ArrayBuffer | null> => {
      return new Promise((resolve) => {
        const ws = wsRef.current
        if (!ws || ws.readyState !== WebSocket.OPEN) {
          resolve(null)
          return
        }

        const handler = (event: MessageEvent) => {
          if (event.data instanceof ArrayBuffer) {
            ws.removeEventListener('message', handler)
            resolve(event.data)
          } else if (typeof event.data === 'string') {
            try {
              const msg = JSON.parse(event.data)
              if (msg.error) {
                ws.removeEventListener('message', handler)
                resolve(null)
              }
            } catch { /* ignore */ }
          }
        }
        ws.addEventListener('message', handler)
        ws.send(
          JSON.stringify({ read_dump_file: path, format: 'wav', tap }),
        )

        setTimeout(() => {
          ws.removeEventListener('message', handler)
          resolve(null)
        }, 30000)
      })
    },
    [],
  )

  return {
    connected,
    availableTaps,
    framesRef,
    vadMetaRef,
    levelsRef,
    onFrame,
    onVadMeta,
    sendCommand,
    listDumps,
    requestDumpFile,
  }
}
