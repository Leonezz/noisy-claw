interface TapSelectorProps {
  availableTaps: Set<string>
  selectedTaps: string[]
  onToggle: (tap: string) => void
}

const TAP_COLORS: Record<string, string> = {
  capture: '#3b82f6',      // blue
  speaker_out: '#f97316',  // orange
  render: '#06b6d4',       // cyan
  aec_out: '#22c55e',      // green
  vad_pass: '#a855f7',     // purple
  tts_out: '#ef4444',      // red
}

export function getTapColor(tap: string): string {
  return TAP_COLORS[tap] ?? '#9ca3af'
}

export function TapSelector({ availableTaps, selectedTaps, onToggle }: TapSelectorProps) {
  const taps = Array.from(availableTaps).sort()

  return (
    <div className="flex flex-wrap gap-2">
      {taps.map((tap) => {
        const active = selectedTaps.includes(tap)
        const color = getTapColor(tap)
        return (
          <button
            key={tap}
            onClick={() => onToggle(tap)}
            className={`px-2 py-1 text-xs font-mono rounded border transition-colors ${
              active
                ? 'border-current bg-gray-800'
                : 'border-gray-700 bg-gray-900 opacity-50'
            }`}
            style={{ color }}
          >
            {tap}
          </button>
        )
      })}
    </div>
  )
}
