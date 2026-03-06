import type { PipelineData } from '../lib/protocol'

const NODE_COLORS: Record<string, string> = {
  capture: '#22c55e',
  aec: '#06b6d4',
  vad: '#f59e0b',
  stt: '#8b5cf6',
  tts: '#ec4899',
  output: '#ef4444',
  topic: '#6366f1',
  ipc_sink: '#64748b',
}

const STATUS_COLORS: Record<string, string> = {
  Created: '#94a3b8',
  Running: '#22c55e',
  Stopped: '#ef4444',
}

interface NodeMetricsGridProps {
  pipelineData: PipelineData
}

export function NodeMetricsGrid({ pipelineData }: NodeMetricsGridProps) {
  const { definition, snapshot } = pipelineData
  if (!definition) return null

  return (
    <div>
      <div className="text-xs text-gray-500 mb-2">Pipeline Nodes</div>
      <div className="grid grid-cols-4 gap-2">
        {definition.nodes.map((node) => {
          const snap = snapshot?.nodes[node.name]
          const color = NODE_COLORS[node.type] ?? '#94a3b8'
          const status = snap?.status ?? 'Created'
          const statusColor = STATUS_COLORS[status] ?? '#94a3b8'

          return (
            <div
              key={node.name}
              className="rounded border border-gray-800 bg-gray-900/50 p-2"
              style={{ borderLeftColor: color, borderLeftWidth: 3 }}
            >
              <div className="flex items-center gap-1.5 mb-0.5">
                <span
                  className="w-1.5 h-1.5 rounded-full flex-shrink-0"
                  style={{ backgroundColor: statusColor }}
                />
                <span className="text-xs font-mono text-gray-200 truncate">
                  {node.name}
                </span>
              </div>
              <div className="text-[10px] text-gray-500 mb-1">{node.type}</div>
              {snap?.metrics && Object.keys(snap.metrics).length > 0 && (
                <div className="space-y-0.5">
                  {Object.entries(snap.metrics).map(([k, v]) => (
                    <div key={k} className="flex justify-between text-[10px]">
                      <span className="text-gray-600">{k}</span>
                      <span className="font-mono text-gray-400">
                        {typeof v === 'number'
                          ? Number.isInteger(v)
                            ? v
                            : (v as number).toFixed(2)
                          : String(v)}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
