import { useCallback, useMemo, useState } from 'react'
import { useTapSocket } from './hooks/useTapSocket'
import { WaveformCanvas } from './components/WaveformCanvas'
import { LevelMeter } from './components/LevelMeter'
import { VadPanel } from './components/VadPanel'
import { TapSelector, getTapColor } from './components/TapSelector'
import { DumpBrowser } from './components/DumpBrowser'
import { PipelineGraph } from './components/PipelineGraph'

const DEFAULT_PORT = 9876

type Tab = 'taps' | 'pipeline'

function getPort(): number {
  const params = new URLSearchParams(window.location.search)
  const p = params.get('port')
  return p ? parseInt(p, 10) : DEFAULT_PORT
}

export function App() {
  const port = useMemo(getPort, [])
  const [tab, setTab] = useState<Tab>('pipeline')
  const [selectedTaps, setSelectedTaps] = useState<string[]>([
    'capture',
    'aec_out',
    'vad_pass',
    'tts_out',
  ])
  const [paused, setPaused] = useState(false)
  const [showDumps, setShowDumps] = useState(false)

  // Subscribe to all taps (server filters are per-message; we filter in UI)
  const subscriptions = useMemo(() => ['*'], [])

  const {
    connected,
    availableTaps,
    onFrame,
    onVadMeta,
    onPipeline,
    sendCommand,
    fetchPipeline,
    subscribePipelineSnapshots,
    unsubscribePipelineSnapshots,
    listDumps,
    requestDumpFile,
  } = useTapSocket({
    url: `ws://127.0.0.1:${port}`,
    subscriptions,
    paused,
  })

  const toggleTap = useCallback((tap: string) => {
    setSelectedTaps((prev) =>
      prev.includes(tap) ? prev.filter((t) => t !== tap) : [...prev, tap],
    )
  }, [])

  return (
    <div className="max-w-5xl mx-auto p-4 space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <h1 className="text-lg font-bold text-gray-200">
          Noisy Claw Audio
        </h1>
        <div className="flex items-center gap-3">
          <span
            className={`text-xs font-mono ${
              connected ? 'text-green-400' : 'text-red-400'
            }`}
          >
            {connected ? `connected :${port}` : 'disconnected'}
          </span>
          {tab === 'taps' && (
            <>
              <button
                onClick={() => setPaused((p) => !p)}
                className={`px-2 py-1 text-xs rounded border ${
                  paused
                    ? 'border-yellow-500 bg-yellow-900/30 text-yellow-300'
                    : 'border-gray-700 bg-gray-900 text-gray-400'
                }`}
              >
                {paused ? 'Resume' : 'Pause'}
              </button>
              <button
                onClick={() => setShowDumps((s) => !s)}
                className={`px-2 py-1 text-xs rounded border ${
                  showDumps
                    ? 'border-blue-500 bg-blue-900/30 text-blue-300'
                    : 'border-gray-700 bg-gray-900 text-gray-400'
                }`}
              >
                Dumps
              </button>
            </>
          )}
        </div>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 border-b border-gray-800">
        {(['pipeline', 'taps'] as Tab[]).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`px-3 py-1.5 text-xs font-medium rounded-t border-b-2 transition-colors ${
              tab === t
                ? 'border-blue-500 text-blue-300 bg-blue-900/20'
                : 'border-transparent text-gray-500 hover:text-gray-300'
            }`}
          >
            {t === 'pipeline' ? 'Pipeline Graph' : 'Audio Taps'}
          </button>
        ))}
      </div>

      {/* Tab content */}
      {tab === 'pipeline' && (
        <PipelineGraph
          onPipeline={onPipeline}
          fetchPipeline={fetchPipeline}
          subscribePipelineSnapshots={subscribePipelineSnapshots}
          unsubscribePipelineSnapshots={unsubscribePipelineSnapshots}
          connected={connected}
        />
      )}

      {tab === 'taps' && (
        <>
          {/* Tap selector */}
          <TapSelector
            availableTaps={availableTaps}
            selectedTaps={selectedTaps}
            onToggle={toggleTap}
          />

          {/* Level meters */}
          <div className="space-y-1">
            {selectedTaps.map((tap) => (
              <LevelMeter
                key={tap}
                tap={tap}
                color={getTapColor(tap)}
                onFrame={onFrame}
              />
            ))}
          </div>

          {/* Waveforms */}
          <div className="space-y-2">
            {selectedTaps.map((tap) => (
              <WaveformCanvas
                key={tap}
                tap={tap}
                color={getTapColor(tap)}
                onFrame={onFrame}
                height={100}
              />
            ))}
          </div>

          {/* VAD panel */}
          <VadPanel onVadMeta={onVadMeta} />

          {/* Dump browser */}
          {showDumps && (
            <div className="border border-gray-800 rounded p-3 bg-gray-900/50">
              <DumpBrowser
                listDumps={listDumps}
                requestDumpFile={requestDumpFile}
                sendCommand={sendCommand}
                connected={connected}
              />
            </div>
          )}
        </>
      )}
    </div>
  )
}
