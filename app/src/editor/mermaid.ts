/// Live Mermaid previews for ```mermaid code blocks (lazy-loaded).
type Mermaid = typeof import('mermaid').default

let loader: Promise<Mermaid> | null = null
let seq = 0

const dark = () => window.matchMedia('(prefers-color-scheme: dark)').matches

function load(): Promise<Mermaid> {
  loader ??= import('mermaid').then(({ default: m }) => m)
  return loader
}

/// Configured per render so diagrams follow a system appearance change.
function configure(m: Mermaid, theme: 'dark' | 'neutral' = dark() ? 'dark' : 'neutral') {
  m.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    theme,
    fontFamily: getComputedStyle(document.body).fontFamily,
  })
}

/// A diagram's SVG in the light theme, for printing.
export async function renderMermaidSvg(content: string): Promise<string> {
  const m = await load()
  configure(m, 'neutral')
  const { svg } = await m.render(`kw-mermaid-${++seq}`, content)
  return svg
}

export function renderMermaidPreview(
  language: string,
  content: string,
  apply: (value: null | string | HTMLElement) => void
): null | undefined {
  if (language.trim().toLowerCase() !== 'mermaid' || !content.trim()) return null
  load()
    .then(async (m) => {
      configure(m)
      const el = document.createElement('div')
      try {
        const { svg } = await m.render(`kw-mermaid-${++seq}`, content)
        el.className = 'kw-mermaid'
        el.innerHTML = svg
      } catch (e) {
        el.className = 'kw-mermaid-error'
        el.textContent = e instanceof Error ? e.message : String(e)
      }
      apply(el)
    })
    .catch(() => apply(null))
  // `undefined` tells the code block to show its loading state and wait for apply().
  return undefined
}
