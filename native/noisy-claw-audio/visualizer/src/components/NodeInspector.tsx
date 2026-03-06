import { useCallback, useState } from 'react'
import type { NodeDefinition, NodeSnapshot } from '../lib/protocol'
import { getStatusColor } from '../lib/colors'

interface NodeInspectorProps {
  nodeName: string
  definition?: NodeDefinition
  snapshot?: NodeSnapshot
  onPropertyChange: (node: string, key: string, value: unknown) => void
  onClose: () => void
}

export function NodeInspector({
  nodeName,
  definition,
  snapshot,
  onPropertyChange,
  onClose,
}: NodeInspectorProps) {
  const status = snapshot?.status ?? 'Created'
  const properties = snapshot?.properties ?? definition?.properties ?? {}
  const metrics = snapshot?.metrics ?? {}

  return (
    <div className="w-64 border-l border-gray-800 bg-gray-900/80 overflow-y-auto flex-shrink-0">
      {/* Header */}
      <div className="flex items-center justify-between px-3 py-2 border-b border-gray-800">
        <div className="min-w-0">
          <div className="font-mono text-sm text-gray-200 truncate">{nodeName}</div>
          <div className="text-[10px] text-gray-500">
            {definition?.type ?? snapshot?.node_type}
          </div>
        </div>
        <div className="flex items-center gap-2 flex-shrink-0">
          <span
            className="w-2.5 h-2.5 rounded-full"
            style={{ backgroundColor: getStatusColor(status) }}
            title={status}
          />
          <button
            onClick={onClose}
            className="text-gray-500 hover:text-gray-300 text-sm leading-none"
          >
            x
          </button>
        </div>
      </div>

      {/* Properties */}
      <div className="px-3 py-2 border-b border-gray-800">
        <div className="text-[10px] text-gray-500 uppercase tracking-wider mb-1.5">
          Properties
        </div>
        {Object.keys(properties).length === 0 ? (
          <div className="text-[10px] text-gray-600 italic">No properties</div>
        ) : (
          <div className="space-y-1.5">
            {Object.entries(properties).map(([key, value]) => (
              <PropertyField
                key={key}
                name={key}
                value={value}
                onChange={(v) => onPropertyChange(nodeName, key, v)}
              />
            ))}
          </div>
        )}
      </div>

      {/* Metrics */}
      <div className="px-3 py-2">
        <div className="text-[10px] text-gray-500 uppercase tracking-wider mb-1.5">
          Metrics
        </div>
        {Object.keys(metrics).length === 0 ? (
          <div className="text-[10px] text-gray-600 italic">No metrics</div>
        ) : (
          <div className="space-y-0.5">
            {Object.entries(metrics).map(([key, value]) => (
              <div key={key} className="flex justify-between text-[10px]">
                <span className="text-gray-500">{key}</span>
                <span className="text-gray-300 font-mono">{formatMetric(value)}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

function PropertyField({
  name,
  value,
  onChange,
}: {
  name: string
  value: unknown
  onChange: (v: unknown) => void
}) {
  if (typeof value === 'boolean') {
    return (
      <div className="flex items-center justify-between">
        <span className="text-[10px] text-gray-400">{name}</span>
        <button
          onClick={() => onChange(!value)}
          className={`w-8 h-4 rounded-full transition-colors relative ${
            value ? 'bg-green-600' : 'bg-gray-700'
          }`}
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

  return (
    <div className="flex items-center justify-between gap-2">
      <span className="text-[10px] text-gray-400 truncate">{name}</span>
      <span className="text-[10px] text-gray-300 font-mono truncate">{String(value)}</span>
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
        <span className="text-[10px] text-gray-400 truncate">{name}</span>
        <input
          autoFocus
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commit()
            if (e.key === 'Escape') setEditing(false)
          }}
          className="w-20 text-[10px] text-right bg-gray-800 border border-gray-600 rounded px-1 py-0.5 font-mono text-gray-200 outline-none focus:border-blue-500"
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
      <span className="text-[10px] text-gray-400 truncate">{name}</span>
      <span className="text-[10px] text-gray-300 font-mono group-hover:text-blue-400">
        {value}
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
