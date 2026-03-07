import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useTapSocket } from './hooks/useTapSocket'
import { WaveformCanvas } from './components/WaveformCanvas'
import { LevelMeter } from './components/LevelMeter'
import { TapSelector } from './components/TapSelector'
import { getTapColor } from './lib/colors'
import { DumpBrowser } from './components/DumpBrowser'
import { PipelineGraph } from './components/PipelineGraph'
import { NodeDashboard } from './components/NodeDashboard'
import { NodeMetricsGrid } from './components/NodeMetricsGrid'
import { GenericStreamView } from './components/GenericStreamView'
import { SpectrogramCanvas } from './components/SpectrogramCanvas'
import { SpectrumCanvas } from './components/SpectrumCanvas'
import type { PipelineData, NodeTypeInfo, DataStreamDescriptor } from './lib/protocol'
import { ThemeContext, getTokens, type Theme } from './lib/theme'

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

const TABS: Tab[] = ['graph', 'dashboard', 'dumps']

function getInitialTheme(): Theme {
  return (localStorage.getItem('theme') as Theme) ?? 'dark'
}

export function App() {
  const port = useMemo(getPort, [])
  const [theme, setTheme] = useState<Theme>(getInitialTheme)
  const tk = getTokens(theme)
  const toggleTheme = useCallback(() => {
    setTheme((prev) => {
      const next = prev === 'dark' ? 'light' : 'dark'
      localStorage.setItem('theme', next)
      return next
    })
  }, [])
  const themeCtx = useMemo(() => ({ theme, toggle: toggleTheme }), [theme, toggleTheme])

  const [tab, setTab] = useState<Tab>(getInitialTab)
  const [selectedTaps, setSelectedTaps] = useState<string[]>([])
  const [paused, setPaused] = useState(false)
  const [selectedNode, setSelectedNode] = useState<string | null>(null)
  const [pipelineData, setPipelineData] = useState<PipelineData>({
    definition: null,
    snapshot: null,
  })
  const [nodeTypeRegistry, setNodeTypeRegistry] = useState<NodeTypeInfo[]>([])
  const [pendingProps, setPendingProps] = useState<
    { node: string; key: string; value: unknown }[]
  >([])

  const subscriptions = useMemo(() => ['*'], [])

  const {
    connected,
    availableTaps,
    onFrame,
    onMetadata,
    onPipeline,
    sendCommand,
    fetchPipeline,
    fetchNodeTypes,
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
    return onPipeline((data) => {
      setPipelineData(data)
      // Auto-select audio taps from data streams on first pipeline load
      const streams = data.definition?.data_streams
      if (streams && selectedTaps.length === 0) {
        const audioTaps = streams
          .filter((s): s is Extract<DataStreamDescriptor, { kind: 'audio' }> => s.kind === 'audio')
          .map((s) => s.name)
        if (audioTaps.length > 0) setSelectedTaps(audioTaps)
      }
    })
  }, [onPipeline])

  useEffect(() => {
    if (connected) {
      fetchPipeline()
      fetchNodeTypes().then(setNodeTypeRegistry)
      subscribePipelineSnapshots(2000)
      return () => unsubscribePipelineSnapshots()
    }
  }, [connected, fetchPipeline, fetchNodeTypes, subscribePipelineSnapshots, unsubscribePipelineSnapshots])

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

  const bufferPropertyChange = useCallback(
    (node: string, key: string, value: unknown) => {
      setPendingProps((prev) => {
        const filtered = prev.filter(
          (p) => !(p.node === node && p.key === key),
        )
        return [...filtered, { node, key, value }]
      })
    },
    [],
  )

  const applyPendingProps = useCallback(() => {
    setPendingProps((current) => {
      for (const { node, key, value } of current) {
        sendCommand({ set_property: { node, key, value } })
      }
      return []
    })
  }, [sendCommand])

  const clearPendingProps = useCallback(() => {
    setPendingProps([])
  }, [])

  const toggleTap = useCallback((tap: string) => {
    setSelectedTaps((prev) =>
      prev.includes(tap) ? prev.filter((t) => t !== tap) : [...prev, tap],
    )
  }, [])

  // Resizable bottom dashboard state
  const [dashboardHeight, setDashboardHeight] = useState(240)
  const dragRef = useRef<{ startY: number; startH: number } | null>(null)

  useEffect(() => {
    function onMouseMove(e: MouseEvent) {
      if (!dragRef.current) return
      const delta = dragRef.current.startY - e.clientY
      const newH = Math.max(120, Math.min(600, dragRef.current.startH + delta))
      setDashboardHeight(newH)
    }
    function onMouseUp() {
      dragRef.current = null
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
    }
    window.addEventListener('mousemove', onMouseMove)
    window.addEventListener('mouseup', onMouseUp)
    return () => {
      window.removeEventListener('mousemove', onMouseMove)
      window.removeEventListener('mouseup', onMouseUp)
    }
  }, [])

  const onDragStart = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      dragRef.current = { startY: e.clientY, startH: dashboardHeight }
      document.body.style.cursor = 'row-resize'
      document.body.style.userSelect = 'none'
    },
    [dashboardHeight],
  )

  const modes = pipelineData.definition?.modes
    ? Object.keys(pipelineData.definition.modes)
    : []
  const currentMode = pipelineData.snapshot?.current_mode ?? null

  const selectedNodeSnapshot = selectedNode
    ? pipelineData.snapshot?.nodes[selectedNode]
    : undefined
  const mergedNodeSnapshot = useMemo(() => {
    if (!selectedNodeSnapshot || !selectedNode) return selectedNodeSnapshot
    const nodeOverrides = pendingProps.filter((p) => p.node === selectedNode)
    if (nodeOverrides.length === 0) return selectedNodeSnapshot
    const mergedProperties = { ...selectedNodeSnapshot.properties }
    for (const { key, value } of nodeOverrides) {
      mergedProperties[key] = value
    }
    return { ...selectedNodeSnapshot, properties: mergedProperties }
  }, [selectedNodeSnapshot, selectedNode, pendingProps])

  const pendingNodeKeys = useMemo(
    () =>
      selectedNode
        ? pendingProps
            .filter((p) => p.node === selectedNode)
            .map((p) => p.key)
        : [],
    [selectedNode, pendingProps],
  )

  return (
    <ThemeContext.Provider value={themeCtx}>
      <div className="h-screen flex flex-col" style={{ backgroundColor: tk.bgPage }}>
        {/* Top bar */}
        <div
          className="flex items-center justify-between flex-shrink-0 px-6"
          style={{ height: 48, backgroundColor: tk.bgSurface, borderBottom: `1px solid ${tk.borderPrimary}` }}
        >
          {/* Left: logo + tabs */}
          <div className="flex items-center gap-4 h-full">
            <span className="font-mono text-sm font-semibold" style={{ color: tk.accentGreen }}>
              ~ noisy_claw
            </span>
            <div className="flex h-full">
              {TABS.map((t) => (
                <button
                  key={t}
                  onClick={() => changeTab(t)}
                  className="px-4 h-full font-mono text-xs font-medium transition-colors"
                  style={{
                    color: tab === t ? tk.accentGreen : tk.textTertiary,
                    borderBottom: tab === t ? `2px solid ${tk.accentGreen}` : '2px solid transparent',
                  }}
                >
                  {t}
                </button>
              ))}
            </div>
          </div>

          {/* Right: theme toggle + mode switcher + connection + pause */}
          <div className="flex items-center gap-3">
            <button
              onClick={toggleTheme}
              className="font-mono text-[10px] px-2 py-1 rounded transition-colors"
              style={{ color: tk.textMuted, border: `1px solid ${tk.borderPrimary}` }}
              title={`Switch to ${theme === 'dark' ? 'light' : 'dark'} theme`}
            >
              {theme === 'dark' ? 'light' : 'dark'}
            </button>

            {modes.length > 0 && (
              <div
                className="flex rounded overflow-hidden"
                style={{ border: `1px solid ${tk.borderPrimary}` }}
              >
                {modes.map((mode) => (
                  <button
                    key={mode}
                    onClick={() => setMode(mode)}
                    className="px-3 py-1.5 font-mono text-[10px] font-medium transition-colors"
                    style={{
                      color: currentMode === mode ? tk.accentGreen : tk.textMuted,
                      backgroundColor: currentMode === mode ? tk.accentGreenBg : undefined,
                    }}
                  >
                    {mode}
                  </button>
                ))}
              </div>
            )}

            <span className="font-mono text-[11px]" style={{ color: connected ? tk.accentGreen : tk.accentError }}>
              {connected ? `● connected :${port}` : '● disconnected'}
            </span>

            {tab === 'dashboard' && (
              <button
                onClick={() => setPaused((p) => !p)}
                className="px-3 py-1 font-mono text-[10px] rounded transition-colors"
                style={{
                  color: paused ? tk.accentWarning : tk.textMuted,
                  border: `1px solid ${paused ? tk.accentWarning + '30' : tk.borderPrimary}`,
                  backgroundColor: paused ? tk.accentWarning + '10' : undefined,
                }}
              >
                {paused ? 'resume' : 'pause'}
              </button>
            )}
          </div>
        </div>

        {/* Main body */}
        <div className="flex-1 min-h-0 flex flex-col">
          {tab === 'graph' && (
            <div className="flex flex-col h-full">
              <div className="flex-1 min-h-0">
                <PipelineGraph
                  pipelineData={pipelineData}
                  onNodeSelect={setSelectedNode}
                  availableNodeTypes={nodeTypeRegistry}
                  sendCommand={sendCommand}
                  pendingPropertyChanges={pendingProps}
                  onApplyProperties={applyPendingProps}
                  onClearProperties={clearPendingProps}
                />
              </div>
              {selectedNode && (
                <>
                  <div
                    onMouseDown={onDragStart}
                    className="flex-shrink-0 cursor-row-resize group flex items-center justify-center"
                    style={{ height: 8, backgroundColor: tk.bgSurface, borderTop: `1px solid ${tk.borderPrimary}` }}
                  >
                    <div
                      className="rounded-sm group-hover:opacity-70 transition-opacity"
                      style={{ width: 40, height: 3, backgroundColor: tk.handleBar }}
                    />
                  </div>
                  <div style={{ height: dashboardHeight }} className="flex-shrink-0">
                    <NodeDashboard
                      nodeName={selectedNode}
                      definition={pipelineData.definition?.nodes.find(
                        (n) => n.name === selectedNode,
                      )}
                      snapshot={mergedNodeSnapshot}
                      dataStreams={
                        pipelineData.definition?.data_streams?.filter(
                          (ds) => ds.node === selectedNode,
                        ) ?? []
                      }
                      onPropertyChange={bufferPropertyChange}
                      pendingKeys={pendingNodeKeys}
                      onFrame={onFrame}
                      onMetadata={onMetadata}
                      onClose={() => setSelectedNode(null)}
                    />
                  </div>
                </>
              )}
            </div>
          )}

          {tab === 'dashboard' && (
            <div className="space-y-4 overflow-y-auto h-full p-4">
              <TapSelector
                availableTaps={availableTaps}
                selectedTaps={selectedTaps}
                onToggle={toggleTap}
              />
              <div className="space-y-1">
                {selectedTaps.map((tap) => (
                  <LevelMeter key={tap} tap={tap} color={getTapColor(tap)} onFrame={onFrame} />
                ))}
              </div>
              <div className="space-y-2">
                {selectedTaps.map((tap) => (
                  <WaveformCanvas key={tap} tap={tap} color={getTapColor(tap)} onFrame={onFrame} height={100} />
                ))}
              </div>
              <div className="space-y-2">
                {selectedTaps.map((tap) => (
                  <SpectrumCanvas key={tap} tap={tap} color={getTapColor(tap)} onFrame={onFrame} height={100} />
                ))}
              </div>
              <div className="space-y-2">
                {selectedTaps.map((tap) => (
                  <SpectrogramCanvas key={tap} tap={tap} color={getTapColor(tap)} onFrame={onFrame} height={200} />
                ))}
              </div>
              {/* Render all non-audio data streams generically */}
              {(pipelineData.definition?.data_streams ?? [])
                .filter((ds) => ds.kind !== 'audio')
                .map((ds) => (
                  <GenericStreamView
                    key={ds.name}
                    descriptor={ds}
                    onFrame={onFrame}
                    onMetadata={onMetadata}
                  />
                ))}
              <NodeMetricsGrid pipelineData={pipelineData} />
            </div>
          )}

          {tab === 'dumps' && (
            <div className="p-4 h-full">
              <div
                className="rounded p-3 h-full"
                style={{ border: `1px solid ${tk.borderPrimary}`, backgroundColor: tk.bgSurface + '80' }}
              >
                <DumpBrowser
                  listDumps={listDumps}
                  requestDumpFile={requestDumpFile}
                  sendCommand={sendCommand}
                  connected={connected}
                />
              </div>
            </div>
          )}
        </div>
      </div>
    </ThemeContext.Provider>
  )
}
