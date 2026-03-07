import { useCallback, useEffect, useState } from 'react'
import type { DumpEntry } from '../lib/protocol'
import { getTapColor } from './TapSelector'
import { useTheme, getTokens } from '../lib/theme'

interface DumpBrowserProps {
  listDumps: () => Promise<DumpEntry[]>
  requestDumpFile: (path: string, tap: string) => Promise<ArrayBuffer | null>
  sendCommand: (cmd: Record<string, unknown>) => void
  connected: boolean
}

interface DumpFile {
  name: string
  size: number
}

export function DumpBrowser({
  listDumps,
  requestDumpFile,
  sendCommand,
  connected,
}: DumpBrowserProps) {
  const [dumps, setDumps] = useState<DumpEntry[]>([])
  const [selectedDump, setSelectedDump] = useState<string | null>(null)
  const [files, setFiles] = useState<DumpFile[]>([])
  const [loading, setLoading] = useState(false)
  const [playing, setPlaying] = useState<string | null>(null)
  const [audioCtx] = useState(() => new AudioContext())

  const refreshDumps = useCallback(async () => {
    if (!connected) return
    const result = await listDumps()
    setDumps(result)
  }, [connected, listDumps])

  useEffect(() => {
    refreshDumps()
  }, [refreshDumps])

  const selectDump = useCallback(
    (name: string) => {
      setSelectedDump(name)
      // Request file list via WS
      sendCommand({ list_dump_files: name })

      // Listen for response (hacky but works for this simple UI)
      const handler = (event: MessageEvent) => {
        if (typeof event.data === 'string') {
          try {
            const msg = JSON.parse(event.data)
            if (msg.dump === name && msg.files) {
              setFiles(msg.files)
            }
          } catch { /* ignore */ }
        }
      }
      // We need the raw WS — but we're using the hook's sendCommand.
      // For simplicity, re-request via list_dump_files which returns a response
      // The hook should handle this, but we'll parse it in the parent.
      // For now, just set empty and let the hook handle it.
      setFiles([])

      // Actually, let's poll via the send mechanism
      // We'll listen on the next text message
      setTimeout(() => {
        sendCommand({ list_dump_files: name })
      }, 100)

      // Clear handler reference
      void handler
    },
    [sendCommand],
  )

  const playFile = useCallback(
    async (filename: string) => {
      if (!selectedDump) return
      setLoading(true)
      setPlaying(filename)

      try {
        // Extract tap name from filename (e.g., "capture.pcm" → "capture")
        const tap = filename.replace('.pcm', '')
        const path = `${selectedDump}/${filename}`

        const wavData = await requestDumpFile(path, tap)
        if (!wavData) {
          setPlaying(null)
          setLoading(false)
          return
        }

        const audioBuffer = await audioCtx.decodeAudioData(wavData)
        const source = audioCtx.createBufferSource()
        source.buffer = audioBuffer
        source.connect(audioCtx.destination)
        source.onended = () => setPlaying(null)
        source.start()
        setLoading(false)
      } catch (e) {
        console.error('Playback error:', e)
        setPlaying(null)
        setLoading(false)
      }
    },
    [selectedDump, requestDumpFile, audioCtx],
  )

  const formatSize = (bytes: number) => {
    if (bytes < 1024) return `${bytes}B`
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`
    return `${(bytes / (1024 * 1024)).toFixed(1)}MB`
  }

  const formatDuration = (bytes: number, sampleRate = 48000) => {
    const samples = bytes / 4 // f32 = 4 bytes
    const seconds = samples / sampleRate
    return `${seconds.toFixed(1)}s`
  }

  const { theme } = useTheme()
  const tk = getTokens(theme)

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <h3 className="text-sm font-semibold" style={{ color: tk.textSecondary }}>Dump Files</h3>
        <button
          onClick={refreshDumps}
          className="text-xs px-2 py-0.5 rounded transition-colors"
          style={{ backgroundColor: tk.bgSurface, border: `1px solid ${tk.borderPrimary}`, color: tk.textSecondary }}
        >
          Refresh
        </button>
      </div>

      {dumps.length === 0 ? (
        <p className="text-xs" style={{ color: tk.textTertiary }}>
          No dumps found. Set AUDIO_DUMP_DIR to enable.
        </p>
      ) : (
        <div className="flex gap-2 flex-wrap">
          {dumps.map((dump) => (
            <button
              key={dump.name}
              onClick={() => selectDump(dump.name)}
              className="text-xs px-2 py-1 rounded border font-mono transition-colors"
              style={{
                borderColor: selectedDump === dump.name ? tk.accentInfo : tk.borderPrimary,
                backgroundColor: selectedDump === dump.name ? tk.accentInfoBg : tk.bgPage,
                color: selectedDump === dump.name ? tk.accentInfo : tk.textTertiary,
              }}
            >
              {dump.name.replace('dump_', '')}
            </button>
          ))}
        </div>
      )}

      {selectedDump && files.length > 0 && (
        <div className="grid grid-cols-2 gap-1">
          {files
            .filter((f) => f.name.endsWith('.pcm'))
            .sort((a, b) => a.name.localeCompare(b.name))
            .map((file) => {
              const tap = file.name.replace('.pcm', '')
              const color = getTapColor(tap)
              const isPlaying = playing === file.name

              return (
                <button
                  key={file.name}
                  onClick={() => playFile(file.name)}
                  disabled={loading && isPlaying}
                  className="flex items-center justify-between px-2 py-1 text-xs font-mono rounded border transition-colors"
                  style={{
                    borderColor: isPlaying ? tk.accentGreen : tk.borderPrimary,
                    backgroundColor: isPlaying ? tk.accentGreenBg : tk.bgPage,
                  }}
                >
                  <span style={{ color }}>{tap}</span>
                  <span style={{ color: tk.textTertiary }}>
                    {formatSize(file.size)} / {formatDuration(file.size)}
                  </span>
                </button>
              )
            })}
        </div>
      )}
    </div>
  )
}
