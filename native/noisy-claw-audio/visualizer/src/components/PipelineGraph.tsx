import { useCallback, useMemo } from 'react'
import {
  ReactFlow,
  type Node,
  type Edge,
  Background,
  Controls,
  MiniMap,
  type NodeTypes,
  Handle,
  Position,
  type NodeProps,
} from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import type { PipelineData, PipelineDefinition, PipelineSnapshot } from '../lib/protocol'

// ── Node type colors ──────────────────────────────────────────────

const NODE_COLORS: Record<string, string> = {
  capture: '#22c55e',  // green
  aec: '#06b6d4',      // cyan
  vad: '#f59e0b',      // amber
  stt: '#8b5cf6',      // violet
  tts: '#ec4899',      // pink
  output: '#ef4444',   // red
  topic: '#6366f1',    // indigo
  ipc_sink: '#64748b', // slate
}

const STATUS_COLORS: Record<string, string> = {
  Created: '#94a3b8',
  Running: '#22c55e',
  Stopped: '#ef4444',
}

function getNodeColor(nodeType: string): string {
  return NODE_COLORS[nodeType] ?? '#94a3b8'
}

// ── Custom pipeline node component ──────────────────────────────

type PipelineNodeData = {
  label: string
  nodeType: string
  status: string
  properties: Record<string, unknown>
  inputPorts: string[]
  outputPorts: string[]
}

function PipelineNodeComponent({ data }: NodeProps<Node<PipelineNodeData>>) {
  const color = getNodeColor(data.nodeType)
  const statusColor = STATUS_COLORS[data.status] ?? '#94a3b8'

  return (
    <div
      className="rounded-lg border-2 bg-gray-900 shadow-lg min-w-[140px]"
      style={{ borderColor: color }}
    >
      {/* Input handles */}
      {data.inputPorts.map((port, i) => (
        <Handle
          key={`in-${port}`}
          type="target"
          position={Position.Left}
          id={port}
          style={{
            top: `${((i + 1) / (data.inputPorts.length + 1)) * 100}%`,
            background: '#64748b',
            width: 8,
            height: 8,
          }}
        />
      ))}

      {/* Header */}
      <div
        className="px-3 py-1.5 text-xs font-bold text-white rounded-t-md"
        style={{ backgroundColor: color + '30' }}
      >
        <div className="flex items-center justify-between gap-2">
          <span>{data.label}</span>
          <span
            className="w-2 h-2 rounded-full"
            style={{ backgroundColor: statusColor }}
            title={data.status}
          />
        </div>
        <div className="text-[10px] font-normal opacity-60">{data.nodeType}</div>
      </div>

      {/* Port labels */}
      <div className="px-2 py-1 text-[10px] text-gray-500 space-y-0.5">
        {data.inputPorts.length > 0 && (
          <div className="flex flex-col">
            {data.inputPorts.map((p) => (
              <span key={p} className="text-left">{'→ '}{p}</span>
            ))}
          </div>
        )}
        {data.outputPorts.length > 0 && (
          <div className="flex flex-col">
            {data.outputPorts.map((p) => (
              <span key={p} className="text-right">{p}{' →'}</span>
            ))}
          </div>
        )}
      </div>

      {/* Output handles */}
      {data.outputPorts.map((port, i) => (
        <Handle
          key={`out-${port}`}
          type="source"
          position={Position.Right}
          id={port}
          style={{
            top: `${((i + 1) / (data.outputPorts.length + 1)) * 100}%`,
            background: color,
            width: 8,
            height: 8,
          }}
        />
      ))}
    </div>
  )
}

const nodeTypes: NodeTypes = {
  pipeline: PipelineNodeComponent,
}

// ── Port metadata (mirrors Rust node port declarations) ──────────

const PORT_INFO: Record<string, { inputs: string[]; outputs: string[] }> = {
  capture:  { inputs: [],                          outputs: ['audio_out'] },
  aec:      { inputs: ['capture_in', 'render_in'], outputs: ['audio_out'] },
  vad:      { inputs: ['audio_in'],                outputs: ['audio_out', 'vad_event_out', 'ipc_event_out', 'barge_in_out'] },
  stt:      { inputs: ['audio_in', 'vad_in'],      outputs: ['ipc_event_out'] },
  tts:      { inputs: [],                          outputs: ['output_msg_out', 'ipc_event_out'] },
  output:   { inputs: ['output_msg_in'],           outputs: ['render_ref_out'] },
  topic:    { inputs: [],                          outputs: ['ipc_event_out'] },
  ipc_sink: { inputs: ['event_in'],               outputs: [] },
}

// ── Layout: auto-position nodes in a pipeline-style layout ──────

const LAYOUT_ORDER = ['capture', 'aec', 'vad', 'stt', 'tts', 'output', 'topic', 'ipc_sink']

function layoutNodes(def: PipelineDefinition, snapshot: PipelineSnapshot | null): Node[] {
  const sorted = [...def.nodes].sort((a, b) => {
    const ai = LAYOUT_ORDER.indexOf(a.type)
    const bi = LAYOUT_ORDER.indexOf(b.type)
    return (ai === -1 ? 99 : ai) - (bi === -1 ? 99 : bi)
  })

  // Arrange in two rows: audio path on top, support nodes below
  const audioPath = ['capture', 'aec', 'vad', 'stt']
  const supportRow = ['tts', 'output', 'topic', 'ipc_sink']

  return sorted.map((nd) => {
    const snap = snapshot?.nodes[nd.name]
    const ports = PORT_INFO[nd.type] ?? { inputs: [], outputs: [] }
    const isAudioPath = audioPath.includes(nd.type)
    const row = isAudioPath ? 0 : 1
    const col = isAudioPath
      ? audioPath.indexOf(nd.type)
      : supportRow.indexOf(nd.type)

    return {
      id: nd.name,
      type: 'pipeline',
      position: { x: col * 220 + 40, y: row * 180 + 40 },
      data: {
        label: nd.name,
        nodeType: nd.type,
        status: snap?.status ?? 'Created',
        properties: snap?.properties ?? nd.properties,
        inputPorts: ports.inputs,
        outputPorts: ports.outputs,
      } satisfies PipelineNodeData,
    }
  })
}

function buildEdges(def: PipelineDefinition): Edge[] {
  return def.links.map((link, i) => {
    const [srcNode, srcPort] = link.from.split(':')
    const [dstNode, dstPort] = link.to.split(':')
    return {
      id: `e-${i}`,
      source: srcNode,
      sourceHandle: srcPort,
      target: dstNode,
      targetHandle: dstPort,
      animated: true,
      style: { stroke: '#475569', strokeWidth: 2 },
    }
  })
}

// ── Main component ──────────────────────────────────────────────

interface PipelineGraphProps {
  pipelineData: PipelineData
  onNodeSelect?: (name: string | null) => void
}

export function PipelineGraph({ pipelineData, onNodeSelect }: PipelineGraphProps) {
  const nodes = useMemo(() => {
    if (!pipelineData.definition) return []
    return layoutNodes(pipelineData.definition, pipelineData.snapshot)
  }, [pipelineData])

  const edges = useMemo(() => {
    if (!pipelineData.definition) return []
    return buildEdges(pipelineData.definition)
  }, [pipelineData.definition])

  const onNodesChange = useCallback(() => {}, [])

  const onNodeClick = useCallback(
    (_: React.MouseEvent, node: Node) => {
      onNodeSelect?.(node.id)
    },
    [onNodeSelect],
  )

  const onPaneClick = useCallback(() => {
    onNodeSelect?.(null)
  }, [onNodeSelect])

  if (!pipelineData.definition) {
    return (
      <div className="flex items-center justify-center h-full text-gray-500 text-sm">
        Loading pipeline...
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col rounded-lg border border-gray-800 bg-gray-950">
      <div className="flex-1">
        <ReactFlow
          nodes={nodes}
          edges={edges}
          nodeTypes={nodeTypes}
          onNodesChange={onNodesChange}
          onNodeClick={onNodeClick}
          onPaneClick={onPaneClick}
          fitView
          proOptions={{ hideAttribution: true }}
          defaultEdgeOptions={{ animated: true }}
        >
          <Background color="#1e293b" gap={20} />
          <Controls
            showInteractive={false}
            className="!bg-gray-800 !border-gray-700 !shadow-lg [&>button]:!bg-gray-800 [&>button]:!border-gray-700 [&>button]:!fill-gray-400"
          />
          <MiniMap
            nodeColor={(n) => getNodeColor((n.data as PipelineNodeData)?.nodeType ?? '')}
            maskColor="rgba(0,0,0,0.7)"
            className="!bg-gray-900 !border-gray-800"
          />
        </ReactFlow>
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-4 px-3 py-1.5 text-[10px] text-gray-500 border-t border-gray-800">
        <span>Pipeline: {pipelineData.definition.name}</span>
        {pipelineData.snapshot?.current_mode && (
          <span>Mode: {pipelineData.snapshot.current_mode}</span>
        )}
        <span>{pipelineData.definition.nodes.length} nodes</span>
        <span>{pipelineData.definition.links.length} links</span>
      </div>
    </div>
  )
}
