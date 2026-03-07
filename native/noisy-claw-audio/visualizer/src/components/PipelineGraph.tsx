import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ReactFlow,
  type Node,
  type Edge,
  type Connection,
  type NodeChange,
  type EdgeChange,
  Background,
  Controls,
  MiniMap,
  Panel,
  type NodeTypes,
  Handle,
  Position,
  type NodeProps,
  applyNodeChanges,
  applyEdgeChanges,
  addEdge,
  useReactFlow,
  ReactFlowProvider,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import type {
  PipelineData,
  PipelineDefinition,
  PipelineSnapshot,
  NodeTypeInfo,
} from '../lib/protocol'
import { getNodeTypeColor, getStatusColor } from '../lib/colors'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { PipelineJsonPanel } from './PipelineJsonPanel'
import { useTheme, getTokens } from '../lib/theme'

/** Extract status string from the tagged-enum status field. */
function normalizeStatus(
  status: string | { status: string; message?: string } | undefined,
): string {
  if (!status) return 'Created'
  if (typeof status === 'string') return status
  return status.status ?? 'Created'
}

// ── Layout constants ───────────────────────────────────────────────

const HEADER_H = 44
const ERROR_H = 26
const ROW_H = 24
const PORT_PAD = 4

// ── Custom pipeline node component ─────────────────────────────────

type PipelineNodeData = {
  label: string
  nodeType: string
  status: string
  lastError?: string
  properties: Record<string, unknown>
  inputPorts: string[]
  outputPorts: string[]
  portTypes: Record<string, string>
}

function PipelineNodeComponent({ data }: NodeProps<Node<PipelineNodeData>>) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const color = getNodeTypeColor(data.nodeType)
  const statusColor = getStatusColor(data.status)
  const isError = data.status.toLowerCase() === 'error' || !!data.lastError
  const maxPorts = Math.max(data.inputPorts.length, data.outputPorts.length)
  const headerOffset = HEADER_H + (data.lastError ? ERROR_H : 0)

  return (
    <div
      className="rounded-lg min-w-[160px]"
      style={{
        border: `${isError ? 3 : 2}px solid ${isError ? '#ef4444' : color}`,
        backgroundColor: isError ? 'rgba(239,68,68,0.06)' : tk.nodeBackground,
        boxShadow: isError ? '0 0 16px rgba(239,68,68,0.25)' : tk.nodeShadow,
      }}
    >
      {/* Header */}
      <div
        className="px-3 py-1.5 text-xs font-bold rounded-t-md flex flex-col justify-center"
        style={{ backgroundColor: color + tk.nodeHeaderAlpha, height: HEADER_H, color: tk.textPrimary }}
      >
        <div className="flex items-center justify-between gap-2">
          <span>{data.label}</span>
          <span
            className={`w-2 h-2 rounded-full flex-shrink-0${isError ? ' animate-pulse' : ''}`}
            style={{ backgroundColor: statusColor }}
            title={data.status}
          />
        </div>
        <div className="text-[10px] font-normal opacity-60">{data.nodeType}</div>
      </div>

      {/* Error banner */}
      {data.lastError && (
        <div
          className="px-2 py-1 text-[10px] truncate"
          style={{ color: '#fca5a5', backgroundColor: 'rgba(127,29,29,0.4)', borderBottom: '1px solid rgba(153,27,27,0.5)' }}
          title={data.lastError}
        >
          {data.lastError}
        </div>
      )}

      {/* Port rows — inputs left, outputs right, same row index */}
      {maxPorts > 0 && (
        <div style={{ padding: `${PORT_PAD}px 0` }}>
          {Array.from({ length: maxPorts }, (_, i) => (
            <div
              key={i}
              style={{ height: ROW_H }}
              className="flex items-center justify-between px-3 gap-4"
            >
              <span className="text-[10px] font-mono whitespace-nowrap" style={{ color: tk.textTertiary }}>
                {i < data.inputPorts.length ? `→ ${data.inputPorts[i]}` : ''}
              </span>
              <span className="text-[10px] font-mono whitespace-nowrap" style={{ color: tk.textTertiary }}>
                {i < data.outputPorts.length ? `${data.outputPorts[i]} →` : ''}
              </span>
            </div>
          ))}
        </div>
      )}

      {/* Input handles — aligned to port rows */}
      {data.inputPorts.map((port, i) => (
        <Handle
          key={`in-${port}`}
          type="target"
          position={Position.Left}
          id={port}
          style={{
            top: headerOffset + PORT_PAD + i * ROW_H + ROW_H / 2,
            background: tk.edgeStroke,
            width: 8,
            height: 8,
          }}
        />
      ))}

      {/* Output handles — aligned to port rows */}
      {data.outputPorts.map((port, i) => (
        <Handle
          key={`out-${port}`}
          type="source"
          position={Position.Right}
          id={port}
          style={{
            top: headerOffset + PORT_PAD + i * ROW_H + ROW_H / 2,
            background: color,
            width: 8,
            height: 8,
          }}
        />
      ))}
    </div>
  )
}

const customNodeTypes: NodeTypes = {
  pipeline: PipelineNodeComponent,
}

// ── Topological layout from link graph ──────────────────────────────

function layoutNodes(
  def: PipelineDefinition,
  snapshot: PipelineSnapshot | null,
): Node<PipelineNodeData>[] {
  const nodeNames = def.nodes.map((n) => n.name)
  const nodeMap = new Map(def.nodes.map((n) => [n.name, n]))
  const nameSet = new Set(nodeNames)

  // Build unique node-level adjacency
  const inEdges = new Map<string, Set<string>>()
  for (const name of nodeNames) {
    inEdges.set(name, new Set())
  }
  for (const link of def.links) {
    const src = link.from.split(':')[0]
    const dst = link.to.split(':')[0]
    if (nameSet.has(src) && nameSet.has(dst)) {
      inEdges.get(dst)!.add(src)
    }
  }

  // Assign column = longest path from any source to this node
  const col = new Map<string, number>()
  function getCol(name: string, visited: Set<string>): number {
    if (col.has(name)) return col.get(name)!
    if (visited.has(name)) return 0
    visited.add(name)
    const preds = inEdges.get(name)
    if (!preds || preds.size === 0) {
      col.set(name, 0)
      return 0
    }
    let maxPred = 0
    for (const pred of preds) {
      maxPred = Math.max(maxPred, getCol(pred, visited) + 1)
    }
    col.set(name, maxPred)
    return maxPred
  }
  for (const name of nodeNames) {
    getCol(name, new Set())
  }

  // Group by column for row positioning
  const byCol = new Map<number, string[]>()
  for (const name of nodeNames) {
    const c = col.get(name) ?? 0
    const group = byCol.get(c) ?? []
    group.push(name)
    byCol.set(c, group)
  }

  return nodeNames.map((name) => {
    const nd = nodeMap.get(name)!
    const snap = snapshot?.nodes[name]
    const ports = nd.ports ?? []
    const c = col.get(name) ?? 0
    const row = byCol.get(c)!.indexOf(name)

    const inputPorts = ports.filter((p) => p.direction === 'in').map((p) => p.name)
    const outputPorts = ports
      .filter((p) => p.direction === 'out')
      .map((p) => p.name)
    const portTypes: Record<string, string> = {}
    for (const p of ports) {
      portTypes[p.name] = p.port_type
    }

    return {
      id: name,
      type: 'pipeline',
      position: { x: c * 340 + 40, y: row * 260 + 40 },
      data: {
        label: name,
        nodeType: nd.type,
        status: normalizeStatus(snap?.status),
        lastError: snap?.last_error,
        properties: snap?.properties ?? nd.properties,
        inputPorts,
        outputPorts,
        portTypes,
      } satisfies PipelineNodeData,
    }
  })
}

function buildEdges(def: PipelineDefinition, edgeStroke: string): Edge[] {
  return def.links.map((link, i) => {
    const [srcNode, srcPort] = link.from.split(':')
    const [dstNode, dstPort] = link.to.split(':')
    return {
      id: `e-${i}`,
      source: srcNode,
      sourceHandle: srcPort,
      target: dstNode,
      targetHandle: dstPort,
      type: 'default',
      animated: true,
      style: { stroke: edgeStroke, strokeWidth: 2, strokeDasharray: '6 4' },
    }
  })
}

function buildDefinitionFromGraph(
  nodes: Node<PipelineNodeData>[],
  edges: Edge[],
  originalDef: PipelineDefinition,
): PipelineDefinition {
  return {
    name: originalDef.name,
    nodes: nodes.map((n) => ({
      name: n.id,
      type: n.data.nodeType,
      properties: n.data.properties,
    })),
    links: edges
      .filter((e) => e.sourceHandle && e.targetHandle)
      .map((e) => ({
        from: `${e.source}:${e.sourceHandle}`,
        to: `${e.target}:${e.targetHandle}`,
      })),
    modes: originalDef.modes ?? {},
  }
}

// ── Add-node dropdown ──────────────────────────────────────────────

function AddNodeDropdown({
  nodeTypes,
  onAdd,
}: {
  nodeTypes: NodeTypeInfo[]
  onAdd: (info: NodeTypeInfo) => void
}) {
  const { theme } = useTheme()
  const tk = getTokens(theme)

  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          className="px-3 py-1.5 text-xs font-medium rounded transition-colors"
          style={{ backgroundColor: tk.bgSurface, color: tk.textSecondary, border: `1px solid ${tk.borderPrimary}` }}
        >
          + Add Node
        </button>
      </DropdownMenu.Trigger>

      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="rounded shadow-xl z-50 min-w-[200px] max-h-[300px] overflow-y-auto"
          style={{ backgroundColor: tk.bgSurface, border: `1px solid ${tk.borderPrimary}` }}
          sideOffset={4}
          align="start"
        >
          {nodeTypes.map((nt) => (
            <DropdownMenu.Item
              key={nt.node_type}
              className="w-full text-left px-3 py-2 text-xs transition-colors outline-none cursor-pointer last:border-0 data-[highlighted]:opacity-80"
              style={{ color: tk.textSecondary, borderBottom: `1px solid ${tk.borderPrimary}50` }}
              onSelect={() => onAdd(nt)}
            >
              <div className="font-mono font-medium">{nt.node_type}</div>
              <div className="text-[10px] mt-0.5" style={{ color: tk.textTertiary }}>
                {nt.description}
              </div>
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}

// ── Main component ──────────────────────────────────────────────────

interface PipelineGraphProps {
  pipelineData: PipelineData
  onNodeSelect?: (name: string | null) => void
  availableNodeTypes?: NodeTypeInfo[]
  sendCommand?: (cmd: Record<string, unknown>) => void
  pendingPropertyChanges?: { node: string; key: string; value: unknown }[]
  onApplyProperties?: () => void
  onClearProperties?: () => void
}

function PipelineGraphInner({
  pipelineData,
  onNodeSelect,
  availableNodeTypes,
  sendCommand,
  pendingPropertyChanges,
  onApplyProperties,
  onClearProperties,
}: PipelineGraphProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const [nodes, setNodes] = useState<Node<PipelineNodeData>[]>([])
  const [edges, setEdges] = useState<Edge[]>([])
  const [dirty, setDirty] = useState(false)
  const [jsonPanelOpen, setJsonPanelOpen] = useState(false)
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null)
  const defRef = useRef<PipelineDefinition | null>(null)
  const reactFlow = useReactFlow()

  // Current definition: derive from graph when dirty, use server def when clean
  const currentDefinition = useMemo(() => {
    if (!defRef.current) return null
    if (dirty) return buildDefinitionFromGraph(nodes, edges, defRef.current)
    return defRef.current
  }, [dirty, nodes, edges])

  // Initialize or update from server pipeline data
  useEffect(() => {
    const def = pipelineData.definition
    if (!def) return

    // Check if definition structure changed (not just snapshot)
    const defJson = JSON.stringify({
      nodes: def.nodes.map((n) => ({ name: n.name, type: n.type })),
      links: def.links,
    })
    const prevJson = defRef.current
      ? JSON.stringify({
          nodes: defRef.current.nodes.map((n) => ({ name: n.name, type: n.type })),
          links: defRef.current.links,
        })
      : null

    if (prevJson === defJson) {
      // Only snapshot changed — update status/metrics in place
      setNodes((prev) =>
        prev.map((node) => {
          const snap = pipelineData.snapshot?.nodes[node.id]
          if (!snap) return node
          return {
            ...node,
            data: {
              ...node.data,
              status: normalizeStatus(snap.status),
              lastError: snap.last_error,
              properties: snap.properties ?? node.data.properties,
            },
          }
        }),
      )
      return
    }

    // Definition changed — rebuild graph
    defRef.current = def
    setNodes(layoutNodes(def, pipelineData.snapshot))
    setEdges(buildEdges(def, tk.edgeStroke))
    setDirty(false)
  }, [pipelineData, tk.edgeStroke])

  // Update edge stroke color when theme changes
  useEffect(() => {
    setEdges((eds) =>
      eds.map((e) => ({
        ...e,
        style: { ...e.style, stroke: tk.edgeStroke },
      })),
    )
  }, [tk.edgeStroke])

  const onNodesChange = useCallback(
    (changes: NodeChange[]) => {
      const removals = changes.filter((c) => c.type === 'remove')
      if (removals.length > 0) {
        setDirty(true)
        const removedIds = new Set(removals.map((c) => c.id))
        setEdges((eds) =>
          eds.filter((e) => !removedIds.has(e.source) && !removedIds.has(e.target)),
        )
      }
      setNodes((nds) => applyNodeChanges(changes, nds) as Node<PipelineNodeData>[])
    },
    [],
  )

  const onEdgesChange = useCallback((changes: EdgeChange[]) => {
    const hasRemoval = changes.some((c) => c.type === 'remove')
    if (hasRemoval) setDirty(true)
    setEdges((eds) => applyEdgeChanges(changes, eds))
  }, [])

  const onConnect = useCallback((connection: Connection) => {
    setEdges((eds) =>
      addEdge(
        {
          ...connection,
          type: 'default',
          animated: true,
          style: { stroke: tk.edgeStroke, strokeWidth: 2, strokeDasharray: '6 4' },
        },
        eds,
      ),
    )
    setDirty(true)
  }, [tk.edgeStroke])

  const isValidConnection = useCallback(
    (connection: Edge | Connection) => {
      if (connection.source === connection.target) return false

      const sourceNode = nodes.find((n) => n.id === connection.source)
      const targetNode = nodes.find((n) => n.id === connection.target)
      if (!sourceNode || !targetNode) return false

      const srcType = sourceNode.data.portTypes[connection.sourceHandle ?? '']
      const tgtType = targetNode.data.portTypes[connection.targetHandle ?? '']

      if (!srcType || !tgtType) return false
      return srcType === tgtType
    },
    [nodes],
  )

  const onNodeClick = useCallback(
    (_: React.MouseEvent, node: Node) => {
      onNodeSelect?.(node.id)
    },
    [onNodeSelect],
  )

  const onEdgeClick = useCallback(
    (_: React.MouseEvent, edge: Edge) => {
      setSelectedEdgeId(edge.id)
      // Visually highlight the clicked edge
      setEdges((eds) =>
        eds.map((e) => ({
          ...e,
          selected: e.id === edge.id,
          style: {
            ...e.style,
            stroke: e.id === edge.id ? tk.accentError : tk.edgeStroke,
            strokeWidth: e.id === edge.id ? 3 : 2,
          },
        })),
      )
    },
    [tk.edgeStroke, tk.accentError],
  )

  const deleteSelectedEdge = useCallback(() => {
    if (!selectedEdgeId) return
    setEdges((eds) => eds.filter((e) => e.id !== selectedEdgeId))
    setSelectedEdgeId(null)
    setDirty(true)
  }, [selectedEdgeId])

  const onPaneClick = useCallback(() => {
    onNodeSelect?.(null)
    // Deselect edge
    setSelectedEdgeId(null)
    setEdges((eds) =>
      eds.map((e) => ({
        ...e,
        selected: false,
        style: { ...e.style, stroke: tk.edgeStroke, strokeWidth: 2 },
      })),
    )
  }, [onNodeSelect, tk.edgeStroke])

  const handleApply = useCallback(() => {
    if (!sendCommand) return
    const hasPending = (pendingPropertyChanges?.length ?? 0) > 0

    if (dirty && defRef.current) {
      // Topology changed — merge pending props into definition
      const mergedNodes = hasPending
        ? nodes.map((node) => {
            const overrides = (pendingPropertyChanges ?? []).filter(
              (p) => p.node === node.id,
            )
            if (overrides.length === 0) return node
            const mergedProps = { ...node.data.properties }
            for (const { key, value } of overrides) {
              mergedProps[key] = value as Record<string, unknown>[string]
            }
            return {
              ...node,
              data: { ...node.data, properties: mergedProps },
            }
          })
        : nodes
      const def = buildDefinitionFromGraph(mergedNodes, edges, defRef.current)
      sendCommand({ load_pipeline: def })
      setDirty(false)
      onClearProperties?.()
    } else if (hasPending) {
      // Only properties changed — send individual set_property commands
      onApplyProperties?.()
    }
  }, [nodes, edges, dirty, sendCommand, pendingPropertyChanges, onApplyProperties, onClearProperties])

  const handleAddNode = useCallback(
    (typeInfo: NodeTypeInfo) => {
      const existing = nodes.map((n) => n.id)
      let name = typeInfo.node_type
      let counter = 2
      while (existing.includes(name)) {
        name = `${typeInfo.node_type}_${counter++}`
      }

      const { x, y } = reactFlow.screenToFlowPosition({
        x: window.innerWidth / 2,
        y: window.innerHeight / 2,
      })

      const inputPorts = typeInfo.ports
        .filter((p) => p.direction === 'in')
        .map((p) => p.name)
      const outputPorts = typeInfo.ports
        .filter((p) => p.direction === 'out')
        .map((p) => p.name)
      const portTypes: Record<string, string> = {}
      for (const p of typeInfo.ports) {
        portTypes[p.name] = p.port_type
      }

      const newNode: Node<PipelineNodeData> = {
        id: name,
        type: 'pipeline',
        position: { x, y },
        data: {
          label: name,
          nodeType: typeInfo.node_type,
          status: 'Created',
          properties: {},
          inputPorts,
          outputPorts,
          portTypes,
        },
      }

      setNodes((nds) => [...nds, newNode])
      setDirty(true)
    },
    [nodes, reactFlow],
  )

  if (!pipelineData.definition) {
    return (
      <div className="flex items-center justify-center h-full text-sm" style={{ color: tk.textTertiary }}>
        Loading pipeline...
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col" style={{ backgroundColor: tk.bgCanvas }}>
      <div className="flex-1 flex min-h-0">
        <div className="flex-1 min-w-0">
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={customNodeTypes}
            onNodesChange={onNodesChange}
            onEdgesChange={onEdgesChange}
            onConnect={onConnect}
            isValidConnection={isValidConnection}
            onNodeClick={onNodeClick}
            onEdgeClick={onEdgeClick}
            onPaneClick={onPaneClick}
            fitView
            proOptions={{ hideAttribution: true }}
            defaultEdgeOptions={{
              type: 'default',
              animated: true,
            }}
            deleteKeyCode={['Backspace', 'Delete']}
            colorMode={theme}
          >
            <Background color={theme === 'dark' ? '#1e293b' : '#D4D4D4'} gap={20} />
            <Controls showInteractive={false} />
            <MiniMap
              nodeColor={(n) =>
                getNodeTypeColor(
                  (n.data as PipelineNodeData)?.nodeType ?? '',
                )
              }
              maskColor={theme === 'dark' ? 'rgba(0,0,0,0.7)' : 'rgba(255,255,255,0.7)'}
            />

            {/* Toolbar */}
            <Panel position="top-left" className="flex items-center gap-2">
              {availableNodeTypes && availableNodeTypes.length > 0 && (
                <AddNodeDropdown
                  nodeTypes={availableNodeTypes}
                  onAdd={handleAddNode}
                />
              )}
              {(dirty || (pendingPropertyChanges?.length ?? 0) > 0) &&
                sendCommand && (
                  <>
                    <button
                      onClick={handleApply}
                      className="px-4 py-1.5 text-xs font-medium rounded transition-colors"
                      style={{
                        backgroundColor: tk.accentGreen,
                        color: theme === 'dark' ? '#0C0C0C' : '#FFFFFF',
                      }}
                    >
                      apply_changes
                    </button>
                    <span
                      className="text-[10px] font-mono"
                      style={{ color: tk.accentWarning }}
                    >
                      ● {dirty ? 'unsaved' : 'properties modified'}
                    </span>
                  </>
                )}
              {selectedEdgeId && (
                <button
                  onClick={deleteSelectedEdge}
                  className="px-3 py-1.5 text-xs font-medium rounded transition-colors"
                  style={{ backgroundColor: tk.accentError + '20', color: tk.accentError, border: `1px solid ${tk.accentError}40` }}
                >
                  cut link
                </button>
              )}
              <button
                onClick={() => setJsonPanelOpen((v) => !v)}
                className="px-3 py-1.5 text-xs font-semibold rounded border transition-colors"
                style={{
                  color: jsonPanelOpen ? tk.accentInfo : tk.textMuted,
                  backgroundColor: jsonPanelOpen ? tk.accentInfoBg : tk.bgSurface,
                  borderColor: jsonPanelOpen ? tk.accentInfoBorder : tk.borderPrimary,
                }}
                title="Toggle JSON config"
              >
                &lt;/&gt;
              </button>
            </Panel>
          </ReactFlow>
        </div>

        {/* JSON config panel */}
        {jsonPanelOpen && currentDefinition && (
          <PipelineJsonPanel
            definition={currentDefinition}
            onClose={() => setJsonPanelOpen(false)}
          />
        )}
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-5 px-4 py-1.5 font-mono text-[10px] flex-shrink-0" style={{ color: tk.textMuted }}>
        <span>pipeline: {pipelineData.definition.name}</span>
        {pipelineData.snapshot?.current_mode && (
          <span>mode: {pipelineData.snapshot.current_mode}</span>
        )}
        <span>{nodes.length} nodes</span>
        <span>{edges.length} links</span>
      </div>
    </div>
  )
}

export function PipelineGraph(props: PipelineGraphProps) {
  return (
    <ReactFlowProvider>
      <PipelineGraphInner {...props} />
    </ReactFlowProvider>
  )
}
