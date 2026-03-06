import type { PipelineData } from '../lib/protocol'
import { getNodeTypeColor, getStatusColor } from '../lib/colors'

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
          const color = getNodeTypeColor(node.type)
          const status = snap?.status ?? 'Created'
          const statusColor = getStatusColor(status)

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
