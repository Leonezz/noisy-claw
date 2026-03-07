import type { PipelineData } from '../lib/protocol'
import { getNodeTypeColor, getStatusColor } from '../lib/colors'
import { useTheme, getTokens } from '../lib/theme'

interface NodeMetricsGridProps {
  pipelineData: PipelineData
}

export function NodeMetricsGrid({ pipelineData }: NodeMetricsGridProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const { definition, snapshot } = pipelineData
  if (!definition) return null

  return (
    <div>
      <div className="text-xs mb-2" style={{ color: tk.textTertiary }}>Pipeline Nodes</div>
      <div className="grid grid-cols-4 gap-2">
        {definition.nodes.map((node) => {
          const snap = snapshot?.nodes[node.name]
          const color = getNodeTypeColor(node.type)
          const rawStatus = snap?.status
          const status =
            typeof rawStatus === 'object' ? rawStatus?.status ?? 'Created' :
            rawStatus ?? 'Created'
          const statusColor = getStatusColor(status)

          return (
            <div
              key={node.name}
              className="rounded p-2"
              style={{
                border: `1px solid ${tk.borderPrimary}`,
                backgroundColor: tk.bgSurface + '80',
                borderLeftColor: color,
                borderLeftWidth: 3,
              }}
            >
              <div className="flex items-center gap-1.5 mb-0.5">
                <span
                  className="w-1.5 h-1.5 rounded-full flex-shrink-0"
                  style={{ backgroundColor: statusColor }}
                />
                <span className="text-xs font-mono truncate" style={{ color: tk.textPrimary }}>
                  {node.name}
                </span>
              </div>
              <div className="text-[10px] mb-1" style={{ color: tk.textTertiary }}>{node.type}</div>
              {snap?.metrics && Object.keys(snap.metrics).length > 0 && (
                <div className="space-y-0.5">
                  {Object.entries(snap.metrics).map(([k, v]) => (
                    <div key={k} className="flex justify-between text-[10px]">
                      <span style={{ color: tk.textMuted }}>{k}</span>
                      <span className="font-mono" style={{ color: tk.textTertiary }}>
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
