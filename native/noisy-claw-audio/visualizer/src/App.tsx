import { useCallback, useEffect, useMemo, useState } from 'react'
import { useTapSocket } from './hooks/useTapSocket'
import { WaveformCanvas } from './components/WaveformCanvas'
import { LevelMeter } from './components/LevelMeter'
import { VadPanel } from './components/VadPanel'
import { TapSelector } from './components/TapSelector'
import { getTapColor } from './lib/colors'
import { DumpBrowser } from './components/DumpBrowser'
import { PipelineGraph } from './components/PipelineGraph'
import { NodeInspector } from './components/NodeInspector'
import { NodeMetricsGrid } from './components/NodeMetricsGrid'
import type { PipelineData } from './lib/protocol'

const DEFAULT_PORT = 9876

type Tab = 'graph' | 'dashboard' | 'dumps'

function getPort(): number {
  const params = new URLSearchParams(window.location.search)
  const p = params.get('port')
  return p ? parseInt(p, 10) : DEFAULT_PORT
}

function getInitialTab(): Tab {
  const hash = window.location.hash.slice(1)
  if (hash === 'graph' || hash === 'dashboard' || hash === 'dumps') return hash
  return 'graph'
}

const TAB_LABELS: Record<Tab, string> = {
  graph: 'Graph',
  dashboard: 'Dashboard',
  dumps: 'Dumps',
}

export function App() {
  const port = useMemo(getPort, [])
  const [tab, setTab] = useState<Tab>(getInitialTab)
  const [selectedTaps, setSelectedTaps] = useState<string[]>([
    'capture',
    'aec_out',
    'vad_pass',
    'tts_out',
  ])
  const [paused, setPaused] = useState(false)
  const [selectedNode, setSelectedNode] = useState<string | null>(null)
  const [pipelineData, setPipelineData] = useState<PipelineData>({
    definition: null,
    snapshot: null,
  })

  const subscriptions = useMemo(() => ['*'], [])

  const {
    connected,
    availableTaps,
    onFrame,
    onMetadata,
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

  // Centralized pipeline state
  useEffect(() => {
    return onPipeline((data) => setPipelineData(data))
  }, [onPipeline])

  useEffect(() => {
    if (connected) {
      fetchPipeline()
      subscribePipelineSnapshots(2000)
      return () => unsubscribePipelineSnapshots()
    }
  }, [connected, fetchPipeline, subscribePipelineSnapshots, unsubscribePipelineSnapshots])

  // Tab <-> URL hash sync
  const changeTab = useCallback((t: Tab) => {
    setTab(t)
    window.location.hash = t
  }, [])

  const setMode = useCallback(
    (mode: string) => {
      sendCommand({ set_mode: mode })
    },
    [sendCommand],
  )

  const setProperty = useCallback(
    (node: string, key: string, value: unknown) => {
      sendCommand({ set_property: { node, key, value } })
    },
    [sendCommand],
  )

  const toggleTap = useCallback((tap: string) => {
    setSelectedTaps((prev) =>
      prev.includes(tap) ? prev.filter((t) => t !== tap) : [...prev, tap],
    )
  }, [])

  const modes = pipelineData.definition?.modes
    ? Object.keys(pipelineData.definition.modes)
    : []
  const currentMode = pipelineData.snapshot?.current_mode ?? null

  return (
    <div className="h-screen flex flex-col p-4 space-y-3 max-w-6xl mx-auto">
      {/* Header */}
      <div className="flex items-center justify-between flex-shrink-0">
        <h1 className="text-lg font-bold text-gray-200">Noisy Claw Audio</h1>
        <div className="flex items-center gap-3">
          {/* Mode switcher */}
          {modes.length > 0 && (
            <div className="flex rounded border border-gray-700 overflow-hidden">
              {modes.map((mode) => (
                <button
                  key={mode}
                  onClick={() => setMode(mode)}
                  className={`px-2.5 py-1 text-[10px] font-mono transition-colors ${
                    currentMode === mode
                      ? 'bg-blue-900/40 text-blue-300'
                      : 'text-gray-500 hover:text-gray-300 hover:bg-gray-800'
                  }`}
                >
                  {mode}
                </button>
              ))}
            </div>
          )}

          {/* Connection status */}
          <span
            className={`text-xs font-mono ${
              connected ? 'text-green-400' : 'text-red-400'
            }`}
          >
            {connected ? `connected :${port}` : 'disconnected'}
          </span>

          {/* Dashboard controls */}
          {tab === 'dashboard' && (
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
          )}
        </div>
      </div>

      {/* Tabs */}
      <div className="flex gap-1 border-b border-gray-800 flex-shrink-0">
        {(['graph', 'dashboard', 'dumps'] as Tab[]).map((t) => (
          <button
            key={t}
            onClick={() => changeTab(t)}
            className={`px-3 py-1.5 text-xs font-medium rounded-t border-b-2 transition-colors ${
              tab === t
                ? 'border-blue-500 text-blue-300 bg-blue-900/20'
                : 'border-transparent text-gray-500 hover:text-gray-300'
            }`}
          >
            {TAB_LABELS[t]}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div className="flex-1 min-h-0">
        {tab === 'graph' && (
          <div className="flex gap-0 h-full">
            <div className="flex-1 min-w-0">
              <PipelineGraph
                pipelineData={pipelineData}
                onNodeSelect={setSelectedNode}
              />
            </div>
            {selectedNode && (
              <NodeInspector
                nodeName={selectedNode}
                definition={pipelineData.definition?.nodes.find(
                  (n) => n.name === selectedNode,
                )}
                snapshot={pipelineData.snapshot?.nodes[selectedNode]}
                onPropertyChange={setProperty}
                onClose={() => setSelectedNode(null)}
              />
            )}
          </div>
        )}

        {tab === 'dashboard' && (
          <div className="space-y-4 overflow-y-auto h-full pr-1">
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
            <VadPanel onMetadata={onMetadata} />

            {/* Node metrics */}
            <NodeMetricsGrid pipelineData={pipelineData} />
          </div>
        )}

        {tab === 'dumps' && (
          <div className="border border-gray-800 rounded p-3 bg-gray-900/50">
            <DumpBrowser
              listDumps={listDumps}
              requestDumpFile={requestDumpFile}
              sendCommand={sendCommand}
              connected={connected}
            />
          </div>
        )}
      </div>
    </div>
  )
}
