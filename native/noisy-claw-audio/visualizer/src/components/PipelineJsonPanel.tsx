import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import CodeMirror from '@uiw/react-codemirror'
import { json } from '@codemirror/lang-json'
import { createTheme } from '@uiw/codemirror-themes'
import { tags as t } from '@lezer/highlight'
import type { PipelineDefinition } from '../lib/protocol'
import { useTheme, getTokens, tokens } from '../lib/theme'

const darkCmTheme = createTheme({
  theme: 'dark',
  settings: {
    background: tokens.dark.bgPanel,
    foreground: tokens.dark.cmForeground,
    caret: '#60A5FA',
    selection: tokens.dark.cmSelection,
    selectionMatch: '#3B82F610',
    lineHighlight: tokens.dark.cmLineHighlight,
    gutterBackground: tokens.dark.bgPanel,
    gutterForeground: tokens.dark.cmGutter,
    gutterBorder: 'transparent',
    fontFamily: 'JetBrains Mono, ui-monospace, monospace',
    fontSize: '11px',
  },
  styles: [
    { tag: t.propertyName, color: tokens.dark.cmForeground },
    { tag: t.string, color: tokens.dark.cmString },
    { tag: t.number, color: tokens.dark.cmNumber },
    { tag: t.bool, color: tokens.dark.cmBool },
    { tag: t.null, color: tokens.dark.cmBool },
    { tag: [t.brace, t.bracket, t.separator], color: tokens.dark.cmBracket },
  ],
})

const lightCmTheme = createTheme({
  theme: 'light',
  settings: {
    background: tokens.light.bgPanel,
    foreground: tokens.light.cmForeground,
    caret: '#2563EB',
    selection: tokens.light.cmSelection,
    selectionMatch: '#2563EB10',
    lineHighlight: tokens.light.cmLineHighlight,
    gutterBackground: tokens.light.bgPanel,
    gutterForeground: tokens.light.cmGutter,
    gutterBorder: 'transparent',
    fontFamily: 'JetBrains Mono, ui-monospace, monospace',
    fontSize: '11px',
  },
  styles: [
    { tag: t.propertyName, color: tokens.light.cmForeground },
    { tag: t.string, color: tokens.light.cmString },
    { tag: t.number, color: tokens.light.cmNumber },
    { tag: t.bool, color: tokens.light.cmBool },
    { tag: t.null, color: tokens.light.cmBool },
    { tag: [t.brace, t.bracket, t.separator], color: tokens.light.cmBracket },
  ],
})

const jsonExtensions = [json()]

interface PipelineJsonPanelProps {
  definition: PipelineDefinition
  onClose: () => void
}

export function PipelineJsonPanel({
  definition,
  onClose,
}: PipelineJsonPanelProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
  const cmTheme = theme === 'dark' ? darkCmTheme : lightCmTheme

  const jsonStr = useMemo(
    () => JSON.stringify(definition, null, 2),
    [definition],
  )

  const handleCopy = useCallback(() => {
    navigator.clipboard.writeText(jsonStr)
  }, [jsonStr])

  // Resizable width
  const [width, setWidth] = useState(420)
  const dragRef = useRef<{ startX: number; startW: number } | null>(null)

  useEffect(() => {
    function onMouseMove(e: MouseEvent) {
      if (!dragRef.current) return
      const delta = dragRef.current.startX - e.clientX
      setWidth(Math.max(280, Math.min(800, dragRef.current.startW + delta)))
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
      dragRef.current = { startX: e.clientX, startW: width }
      document.body.style.cursor = 'col-resize'
      document.body.style.userSelect = 'none'
    },
    [width],
  )

  return (
    <div className="flex h-full flex-shrink-0">
      {/* Resize handle (left edge) */}
      <div
        onMouseDown={onDragStart}
        className="w-1 flex-shrink-0 cursor-col-resize group relative"
        style={{ backgroundColor: tk.bgSurface, borderLeft: `1px solid ${tk.borderPrimary}` }}
      >
        <div
          className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 rounded-full transition-colors"
          style={{ width: 3, height: 40, backgroundColor: tk.handleBar }}
        />
      </div>

      {/* Panel */}
      <div
        className="flex flex-col h-full"
        style={{ width, backgroundColor: tk.bgPanel }}
      >
        {/* Header */}
        <div
          className="flex items-center justify-between px-3.5 py-2 flex-shrink-0"
          style={{ borderBottom: `1px solid ${tk.borderPrimary}` }}
        >
          <div className="flex items-center gap-2">
            <span className="font-mono text-[10px] font-semibold" style={{ color: tk.accentInfo }}>&lt;/&gt;</span>
            <span className="font-mono text-[11px] font-medium" style={{ color: tk.textSecondary }}>pipeline.json</span>
            <span
              className="font-mono text-[8px] rounded px-1.5 py-0.5"
              style={{ color: tk.textMuted, backgroundColor: tk.borderPrimary }}
            >
              read-only
            </span>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={handleCopy}
              className="font-mono text-[9px] rounded px-2 py-1 transition-colors"
              style={{ color: tk.textTertiary, border: `1px solid ${tk.borderSecondary}` }}
            >
              copy
            </button>
            <button
              onClick={onClose}
              className="font-mono text-sm leading-none transition-colors"
              style={{ color: tk.textMuted }}
            >
              ×
            </button>
          </div>
        </div>

        {/* CodeMirror body */}
        <div className="flex-1 overflow-auto">
          <CodeMirror
            value={jsonStr}
            theme={cmTheme}
            extensions={jsonExtensions}
            readOnly
            editable={false}
            basicSetup={{
              lineNumbers: true,
              foldGutter: true,
              highlightActiveLine: false,
              highlightSelectionMatches: true,
              searchKeymap: true,
              bracketMatching: true,
            }}
            style={{ height: '100%' }}
          />
        </div>
      </div>
    </div>
  )
}
