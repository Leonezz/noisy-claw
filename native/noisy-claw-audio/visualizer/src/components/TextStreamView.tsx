import { useEffect, useRef, useState } from 'react'
import type { MetadataEvent, FieldDescriptor } from '../lib/protocol'
import { useTheme, getTokens } from '../lib/theme'
import { getTapColor } from '../lib/colors'

interface TextStreamViewProps {
  streamName: string
  fields: FieldDescriptor[]
  onMetadata: (listener: (meta: MetadataEvent) => void) => () => void
  maxEntries?: number
}

interface TextEntry {
  text: string
  isFinal: boolean
  time: number
}

export function TextStreamView({
  streamName,
  fields,
  onMetadata,
  maxEntries = 50,
}: TextStreamViewProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const [entries, setEntries] = useState<TextEntry[]>([])
  const containerRef = useRef<HTMLDivElement>(null)

  // Find the string field and optional is_final bool field
  const textField = fields.find((f) => f.field_type === 'string')
  const finalField = fields.find((f) => f.name === 'is_final' && f.field_type === 'bool')

  useEffect(() => {
    return onMetadata((meta) => {
      if (meta.stream !== streamName) return
      const text = textField ? meta.fields[textField.name] : null
      if (typeof text !== 'string' || text.trim().length === 0) return
      const isFinal = finalField ? meta.fields[finalField.name] === true : true
      setEntries((prev) => {
        const next = [...prev, { text: text.trim(), isFinal, time: Date.now() }]
        return next.length > maxEntries ? next.slice(-maxEntries) : next
      })
    })
  }, [onMetadata, streamName, textField?.name, finalField?.name, maxEntries])

  // Auto-scroll
  useEffect(() => {
    const el = containerRef.current
    if (el) el.scrollTop = el.scrollHeight
  }, [entries])

  const streamColor = getTapColor(streamName)

  return (
    <div>
      <div className="text-[10px] mb-1 font-medium" style={{ color: streamColor }}>
        {streamName}
      </div>
      <div
        ref={containerRef}
        className="max-h-[100px] overflow-y-auto rounded p-2 space-y-0.5"
        style={{ border: `1px solid ${tk.borderPrimary}`, backgroundColor: tk.bgPage }}
      >
        {entries.length === 0 ? (
          <div className="text-[10px] italic" style={{ color: tk.textMuted }}>
            Waiting for data...
          </div>
        ) : (
          entries.map((entry, i) => (
            <div
              key={i}
              className="text-[11px] font-mono"
              style={{
                color: entry.isFinal ? tk.textPrimary : tk.textTertiary,
                fontStyle: entry.isFinal ? 'normal' : 'italic',
              }}
            >
              {entry.text}
            </div>
          ))
        )}
      </div>
    </div>
  )
}
