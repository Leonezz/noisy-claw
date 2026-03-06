import { getTapColor } from '../lib/colors'

interface TapSelectorProps {
  availableTaps: Set<string>
  selectedTaps: string[]
  onToggle: (tap: string) => void
}

export { getTapColor }

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
