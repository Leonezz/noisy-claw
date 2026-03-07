import { getTapColor } from '../lib/colors'
import { useTheme, getTokens } from '../lib/theme'

interface TapSelectorProps {
  availableTaps: Set<string>
  selectedTaps: string[]
  onToggle: (tap: string) => void
}

export { getTapColor }

export function TapSelector({ availableTaps, selectedTaps, onToggle }: TapSelectorProps) {
  const { theme } = useTheme()
  const tk = getTokens(theme)
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
            className="px-2 py-1 text-xs font-mono rounded border transition-colors"
            style={{
              color,
              borderColor: active ? color : tk.borderPrimary,
              backgroundColor: active ? tk.bgSurface : tk.bgPage,
              opacity: active ? 1 : 0.5,
            }}
          >
            {tap}
          </button>
        )
      })}
    </div>
  )
}
