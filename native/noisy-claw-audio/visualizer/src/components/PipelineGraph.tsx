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
import { getNodeTypeColor, getStatusColor } from '../lib/colors'

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
  const color = getNodeTypeColor(data.nodeType)
  const statusColor = getStatusColor(data.status)

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

// ── Topological layout from link graph ──────────────────────────

function layoutNodes(def: PipelineDefinition, snapshot: PipelineSnapshot | null): Node[] {
  const nodeNames = def.nodes.map((n) => n.name)
  const nodeMap = new Map(def.nodes.map((n) => [n.name, n]))
  const nameSet = new Set(nodeNames)

  // Build unique node-level adjacency
  const outEdges = new Map<string, Set<string>>()
  const inEdges = new Map<string, Set<string>>()
  for (const name of nodeNames) {
    outEdges.set(name, new Set())
    inEdges.set(name, new Set())
  }
  for (const link of def.links) {
    const src = link.from.split(':')[0]
    const dst = link.to.split(':')[0]
    if (nameSet.has(src) && nameSet.has(dst)) {
      outEdges.get(src)!.add(dst)
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

    return {
      id: name,
      type: 'pipeline',
      position: { x: c * 220 + 40, y: row * 180 + 40 },
      data: {
        label: name,
        nodeType: nd.type,
        status: snap?.status ?? 'Created',
        properties: snap?.properties ?? nd.properties,
        inputPorts: ports.filter((p) => p.direction === 'in').map((p) => p.name),
        outputPorts: ports.filter((p) => p.direction === 'out').map((p) => p.name),
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
            nodeColor={(n) => getNodeTypeColor((n.data as PipelineNodeData)?.nodeType ?? '')}
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
