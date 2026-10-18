/// Live Mermaid previews for ```mermaid code blocks (lazy-loaded).
type Mermaid = typeof import('mermaid').default

let loader: Promise<Mermaid> | null = null
let seq = 0

const dark = () => window.matchMedia('(prefers-color-scheme: dark)').matches

function load(): Promise<Mermaid> {
  loader ??= import('mermaid').then(({ default: m }) => {
    m.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      theme: dark() ? 'dark' : 'neutral',
      fontFamily: getComputedStyle(document.body).fontFamily,
    })
    return m
  })
  return loader
}

export function renderMermaidPreview(
  language: string,
  content: string,
  apply: (value: null | string | HTMLElement) => void
): null | undefined {
  if (language.trim().toLowerCase() !== 'mermaid' || !content.trim()) return null
  load()
    .then(async (m) => {
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
