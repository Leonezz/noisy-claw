const NODE_TYPE_PALETTE = [
  '#22c55e', '#06b6d4', '#f59e0b', '#8b5cf6',
  '#ec4899', '#ef4444', '#6366f1', '#64748b',
  '#3b82f6', '#f97316', '#a855f7', '#10b981',
]

const TAP_PALETTE = [
  '#3b82f6', '#22c55e', '#a855f7', '#ef4444',
  '#06b6d4', '#f97316', '#ec4899', '#f59e0b',
]

const STATUS_COLORS: Record<string, string> = {
  created: '#94a3b8',
  running: '#22c55e',
  stopped: '#ef4444',
  paused: '#f59e0b',
  error: '#ef4444',
}

const typeColorMap = new Map<string, string>()
const tapColorMap = new Map<string, string>()

export function getNodeTypeColor(nodeType: string): string {
  let color = typeColorMap.get(nodeType)
  if (!color) {
    color = NODE_TYPE_PALETTE[typeColorMap.size % NODE_TYPE_PALETTE.length]
    typeColorMap.set(nodeType, color)
  }
  return color
}

export function getTapColor(tap: string): string {
  let color = tapColorMap.get(tap)
  if (!color) {
    color = TAP_PALETTE[tapColorMap.size % TAP_PALETTE.length]
    tapColorMap.set(tap, color)
  }
  return color
}

export function getStatusColor(status: string): string {
  const key = typeof status === 'object' ? (status as any)?.status ?? '' : status
  return STATUS_COLORS[key.toLowerCase()] ?? '#94a3b8'
}
