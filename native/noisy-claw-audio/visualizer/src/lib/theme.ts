import { createContext, useContext } from 'react'

export type Theme = 'dark' | 'light'

export const ThemeContext = createContext<{
  theme: Theme
  toggle: () => void
}>({ theme: 'dark', toggle: () => {} })

export function useTheme() {
  return useContext(ThemeContext)
}

/** All semantic tokens used across the app, keyed by theme. */
export const tokens = {
  dark: {
    bgPage: '#0C0C0C',
    bgSurface: '#171717',
    bgCanvas: '#0A0A0A',
    bgPanel: '#111111',
    bgInput: '#1A1A1A',
    borderPrimary: '#262626',
    borderSecondary: '#333333',
    textPrimary: '#E5E5E5',
    textSecondary: '#A3A3A3',
    textTertiary: '#737373',
    textMuted: '#525252',
    accentGreen: '#22C55E',
    accentGreenBg: '#22C55E18',
    accentError: '#EF4444',
    accentInfo: '#3B82F6',
    accentInfoBg: '#3B82F630',
    accentInfoBorder: '#3B82F660',
    accentWarning: '#F59E0B',
    edgeStroke: '#475569',
    handleBar: '#333333',
    // CodeMirror
    cmForeground: '#A3A3A3',
    cmGutter: '#333333',
    cmString: '#C4B5FD',
    cmNumber: '#4ADE80',
    cmBool: '#FBBF24',
    cmBracket: '#525252',
    cmSelection: '#3B82F620',
    cmLineHighlight: '#ffffff05',
    // Graph node
    nodeBackground: '#111827',
    nodeHeaderAlpha: '30',
    nodeShadow: '0 1px 4px rgba(0,0,0,0.3)',
  },
  light: {
    bgPage: '#FAFAFA',
    bgSurface: '#FFFFFF',
    bgCanvas: '#F5F5F5',
    bgPanel: '#FFFFFF',
    bgInput: '#F0F0F0',
    borderPrimary: '#E5E5E5',
    borderSecondary: '#D4D4D4',
    textPrimary: '#171717',
    textSecondary: '#525252',
    textTertiary: '#737373',
    textMuted: '#A3A3A3',
    accentGreen: '#16A34A',
    accentGreenBg: '#16A34A0C',
    accentError: '#EF4444',
    accentInfo: '#2563EB',
    accentInfoBg: '#2563EB20',
    accentInfoBorder: '#2563EB40',
    accentWarning: '#F59E0B',
    edgeStroke: '#94A3B8',
    handleBar: '#D4D4D4',
    // CodeMirror
    cmForeground: '#525252',
    cmGutter: '#D4D4D4',
    cmString: '#7C3AED',
    cmNumber: '#16A34A',
    cmBool: '#D97706',
    cmBracket: '#A3A3A3',
    cmSelection: '#2563EB20',
    cmLineHighlight: '#00000005',
    // Graph node
    nodeBackground: '#F7F7F7',
    nodeHeaderAlpha: '20',
    nodeShadow: '0 1px 4px rgba(0,0,0,0.08)',
  },
} as const

export type ThemeTokens = { [K in keyof (typeof tokens)['dark']]: string }

export function getTokens(theme: Theme): ThemeTokens {
  return tokens[theme]
}
