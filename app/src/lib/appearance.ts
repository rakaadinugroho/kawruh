/// Page font and text size (Settings → Appearance). Applied as CSS variables
/// on <html>; the editor, page titles and PDF export read `--doc-font` and
/// `--doc-size`. Saved in the app's settings, and cached in localStorage so the
/// first paint already uses them.

export type DocFont = 'serif' | 'sans' | 'mono'
export type DocSize = 'small' | 'default' | 'large'
export type Appearance = { font: DocFont; size: DocSize }

export const DEFAULT_APPEARANCE: Appearance = { font: 'serif', size: 'default' }

const SIZES: Record<DocSize, string> = { small: '15px', default: '17px', large: '19px' }
const CACHE_KEY = 'kawruh.appearance'

export function parseAppearance(font: unknown, size: unknown): Appearance {
  return {
    font: font === 'sans' || font === 'mono' ? font : 'serif',
    size: size === 'small' || size === 'large' ? size : 'default',
  }
}

export function applyAppearance(a: Appearance) {
  const style = document.documentElement.style
  style.setProperty('--doc-font', `var(--font-${a.font})`)
  style.setProperty('--doc-size', SIZES[a.size])
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(a))
  } catch {
    // Storage may be unavailable; the saved setting still applies after load.
  }
}

export function cachedAppearance(): Appearance {
  try {
    const raw = JSON.parse(localStorage.getItem(CACHE_KEY) ?? '{}')
    return parseAppearance(raw.font, raw.size)
  } catch {
    return DEFAULT_APPEARANCE
  }
}
