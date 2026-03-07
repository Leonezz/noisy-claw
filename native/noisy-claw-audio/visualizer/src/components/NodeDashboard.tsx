import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  AudioFrame,
  MetadataEvent,
  NodeDefinition,
  NodeSnapshot,
  DataStreamDescriptor,
} from '../lib/protocol'
import { getStatusColor } from '../lib/colors'
import { GenericStreamView } from './GenericStreamView'
import { useTheme, getTokens } from '../lib/theme'

// ── Node Dashboard ─────────────────────────────────────────────────

interface NodeDashboardProps {
  nodeName: string
  definition?: NodeDefinition
  snapshot?: NodeSnapshot
  dataStreams: DataStreamDescriptor[]
  onPropertyChange: (node: string, key: string, value: unknown) => void
  pendingKeys?: string[]
  onFrame: (listener: (tap: string, frame: AudioFrame) => void) => () => void
  onMetadata: (listener: (meta: MetadataEvent) => void) => () => void
  onClose: () => void
}

export function NodeDashboard({
  nodeName,
  definition,
  snapshot,
  dataStreams,
  onPropertyChange,
  pendingKeys,
  onFrame,
  onMetadata,
  onClose,
}: NodeDashboardProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const [panelWidth, setPanelWidth] = useState(220)
  const dragRef = useRef<{ startX: number; startW: number } | null>(null)

  useEffect(() => {
    function onMouseMove(e: MouseEvent) {
      if (!dragRef.current) return
      const delta = e.clientX - dragRef.current.startX
      setPanelWidth(Math.max(140, Math.min(400, dragRef.current.startW + delta)))
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
      dragRef.current = { startX: e.clientX, startW: panelWidth }
      document.body.style.cursor = 'col-resize'
      document.body.style.userSelect = 'none'
    },
    [panelWidth],
  )

  const rawStatus = snapshot?.status
  const status =
    typeof rawStatus === 'object' ? rawStatus?.status ?? 'Created' :
    rawStatus ?? 'Created'
  const nodeType = definition?.type ?? snapshot?.node_type ?? ''
  const properties = snapshot?.properties ?? definition?.properties ?? {}
  const metrics = snapshot?.metrics ?? {}

  return (
    <div className="overflow-y-auto h-full" style={{ backgroundColor: tk.bgSurface, borderTop: `1px solid ${tk.borderPrimary}` }}>
      {/* Header */}
      <div
        className="flex items-center justify-between px-4 py-2 sticky top-0 z-10"
        style={{ backgroundColor: tk.bgSurface, borderBottom: `1px solid ${tk.borderPrimary}` }}
      >
        <div className="flex items-center gap-2.5">
          <span
            className="w-2 h-2 rounded-full flex-shrink-0"
            style={{ backgroundColor: getStatusColor(status) }}
            title={status}
          />
          <span className="font-mono text-xs font-medium" style={{ color: tk.textPrimary }}>{nodeName}</span>
          <span className="font-mono text-[10px]" style={{ color: tk.textMuted }}>// {nodeType}</span>
        </div>
        <button
          onClick={onClose}
          className="font-mono text-[10px] px-2 py-1 rounded transition-colors"
          style={{ color: tk.textMuted }}
        >
          close
        </button>
      </div>

      <div className="flex h-full">
        {/* Left: Properties + Metrics — resizable */}
        <div className="flex-shrink-0 relative" style={{ width: panelWidth, borderRight: `1px solid ${tk.borderPrimary}` }}>
          {/* Properties */}
          <div className="px-3.5 py-2.5" style={{ borderBottom: `1px solid ${tk.borderPrimary}` }}>
            <div className="text-[10px] uppercase tracking-wider mb-1.5" style={{ color: tk.textTertiary }}>
              Properties
            </div>
            {Object.keys(properties).length === 0 ? (
              <div className="text-[10px] italic" style={{ color: tk.textMuted }}>
                No properties
              </div>
            ) : (
              <div className="space-y-1.5">
                {Object.entries(properties).map(([key, value]) => (
                  <div
                    key={key}
                    style={
                      pendingKeys?.includes(key)
                        ? {
                            borderLeft: `2px solid ${tk.accentWarning}`,
                            paddingLeft: 4,
                          }
                        : undefined
                    }
                  >
                    <PropertyField
                      name={key}
                      value={value}
                      onChange={(v) => onPropertyChange(nodeName, key, v)}
                    />
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* Metrics */}
          <div className="px-3.5 py-2.5">
            <div className="text-[10px] uppercase tracking-wider mb-1.5" style={{ color: tk.textTertiary }}>
              Metrics
            </div>
            {Object.keys(metrics).length === 0 ? (
              <div className="text-[10px] italic" style={{ color: tk.textMuted }}>
                No metrics
              </div>
            ) : (
              <div className="space-y-0.5">
                {Object.entries(metrics).map(([key, value]) => (
                  <div key={key} className="flex justify-between text-[10px]">
                    <span style={{ color: tk.textTertiary }}>{key}</span>
                    <span className="font-mono" style={{ color: tk.textSecondary }}>
                      {formatMetric(value)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
          {/* Resize handle */}
          <div
            onMouseDown={onDragStart}
            className="absolute top-0 right-0 w-1.5 h-full cursor-col-resize group"
            style={{ zIndex: 10 }}
          >
            <div
              className="w-px h-full mx-auto group-hover:opacity-70 transition-opacity"
              style={{ backgroundColor: tk.handleBar }}
            />
          </div>
        </div>

        {/* Right: Data stream visualizations */}
        <div className="flex-1 min-w-0 p-3 space-y-2.5 flex flex-col">
          {dataStreams.length === 0 && (
            <div className="flex items-center justify-center h-20 text-[10px] italic" style={{ color: tk.textMuted }}>
              No data streams for this node
            </div>
          )}

          {dataStreams.map((ds) => (
            <GenericStreamView
              key={ds.name}
              descriptor={ds}
              onFrame={onFrame}
              onMetadata={onMetadata}
            />
          ))}
        </div>
      </div>
    </div>
  )
}

// ── Property editing ────────────────────────────────────────────────

function PropertyField({
  name,
  value,
  onChange,
}: {
  name: string
  value: unknown
  onChange: (v: unknown) => void
}) {
  const { theme } = useTheme()
  const tk = getTokens(theme)

  if (typeof value === 'boolean') {
    return (
      <div className="flex items-center justify-between">
        <span className="text-[10px]" style={{ color: tk.textTertiary }}>{name}</span>
        <button
          onClick={() => onChange(!value)}
          className="w-8 h-4 rounded-full transition-colors relative"
          style={{ backgroundColor: value ? tk.accentGreen : tk.borderSecondary }}
        >
          <div
            className={`w-3 h-3 rounded-full bg-white absolute top-0.5 transition-all ${
              value ? 'left-4' : 'left-0.5'
            }`}
          />
        </button>
      </div>
    )
  }

  if (typeof value === 'number') {
    return <NumberField name={name} value={value} onChange={onChange} />
  }

  if (typeof value === 'string') {
    return <StringField name={name} value={value} onChange={onChange} />
  }

  return (
    <div className="flex items-center justify-between gap-2">
      <span className="text-[10px] truncate" style={{ color: tk.textTertiary }}>{name}</span>
      <span className="text-[10px] font-mono truncate" style={{ color: tk.textSecondary }}>
        {String(value)}
      </span>
    </div>
  )
}

function NumberField({
  name,
  value,
  onChange,
}: {
  name: string
  value: number
  onChange: (v: unknown) => void
}) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(String(value))

  const commit = useCallback(() => {
    const num = parseFloat(draft)
    if (!isNaN(num)) onChange(num)
    setEditing(false)
  }, [draft, onChange])

  if (editing) {
    return (
      <div className="flex items-center justify-between gap-2">
        <span className="text-[10px] truncate" style={{ color: tk.textTertiary }}>{name}</span>
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commit()
            if (e.key === 'Escape') setEditing(false)
          }}
          className="w-20 text-[10px] text-right rounded px-1 py-0.5 font-mono outline-none"
          style={{
            backgroundColor: tk.bgInput,
            border: `1px solid ${tk.borderSecondary}`,
            color: tk.textPrimary,
          }}
        />
      </div>
    )
  }

  return (
    <div
      className="flex items-center justify-between gap-2 cursor-pointer group"
      onClick={() => {
        setEditing(true)
        setDraft(String(value))
      }}
    >
      <span className="text-[10px] truncate" style={{ color: tk.textTertiary }}>{name}</span>
      <span className="text-[10px] font-mono" style={{ color: tk.textSecondary }}>
        {value}
      </span>
    </div>
  )
}

function StringField({
  name,
  value,
  onChange,
}: {
  name: string
  value: string
  onChange: (v: unknown) => void
}) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(value)

  const commit = useCallback(() => {
    if (draft !== value) onChange(draft)
    setEditing(false)
  }, [draft, value, onChange])

  if (editing) {
    return (
      <div className="flex flex-col gap-0.5">
        <span className="text-[10px]" style={{ color: tk.textTertiary }}>{name}</span>
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commit()
            if (e.key === 'Escape') setEditing(false)
          }}
          className="w-full text-[10px] rounded px-1 py-0.5 font-mono outline-none"
          style={{
            backgroundColor: tk.bgInput,
            border: `1px solid ${tk.borderSecondary}`,
            color: tk.textPrimary,
          }}
        />
      </div>
    )
  }

  return (
    <div
      className="flex flex-col gap-0.5 cursor-pointer group"
      onClick={() => {
        setEditing(true)
        setDraft(value)
      }}
    >
      <span className="text-[10px]" style={{ color: tk.textTertiary }}>{name}</span>
      <span className="text-[10px] font-mono truncate" style={{ color: tk.textSecondary }}>
        {value || '(empty)'}
      </span>
    </div>
  )
}

function formatMetric(value: unknown): string {
  if (typeof value === 'number') {
    return Number.isInteger(value) ? String(value) : value.toFixed(3)
  }
  return String(value)
}
