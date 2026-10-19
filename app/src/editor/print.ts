/// Lays out pages for PDF export. Each page's markdown goes through the editor's
/// own schema (so it reads like the editor), then diagrams, canvases and link
/// cards are swapped for their printed form. The result sits in `.kw-print`,
/// hidden on screen and the only thing visible in print (see styles.css).
import { Editor, parserCtx, rootCtx, schemaCtx } from '@milkdown/kit/core'
import { commonmark } from '@milkdown/kit/preset/commonmark'
import { gfm } from '@milkdown/kit/preset/gfm'
import { DOMSerializer } from '@milkdown/kit/prose/model'
import type { LinkMeta, PrintPage } from '../lib/ipc'
import { fillLinkCard } from './link-view'
import { renderMermaidSvg } from './mermaid'
import { kawruhSchema } from './schema'

let headless: Promise<Editor> | null = null

/// A parser/serializer pair without any editing UI.
function editor() {
  headless ??= Editor.make()
    .config((ctx) => ctx.set(rootCtx, document.createElement('div')))
    .use(commonmark)
    .use(gfm)
    .use(kawruhSchema)
    .create()
  return headless
}

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string) {
  const e = document.createElement(tag)
  e.className = className
  if (text) e.textContent = text
  return e
}

async function body(page: PrintPage): Promise<HTMLElement> {
  const ed = await editor()
  const doc = ed.ctx.get(parserCtx)(page.markdown)
  const out = el('div', 'kw-print-body')
  out.append(DOMSerializer.fromSchema(ed.ctx.get(schemaCtx)).serializeFragment(doc.content))

  for (const pre of Array.from(out.querySelectorAll('pre[data-language="mermaid"]'))) {
    const diagram = el('div', 'kw-mermaid')
    try {
      diagram.innerHTML = await renderMermaidSvg(pre.textContent ?? '')
      pre.replaceWith(diagram)
    } catch {
      // Unparseable diagrams print as their source.
    }
  }
  for (const node of Array.from(out.querySelectorAll<HTMLElement>('div[data-kawruh-canvas]'))) {
    const svg = page.canvases[node.dataset.kawruhCanvas ?? '']
    const box = el('div', 'kw-print-canvas')
    if (svg) {
      const image = new Image()
      image.alt = 'Canvas drawing'
      image.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`
      box.append(image)
    } else {
      box.append(el('span', 'kw-canvas-empty', 'Empty canvas'))
    }
    node.replaceWith(box)
  }
  for (const node of Array.from(out.querySelectorAll<HTMLElement>('div[data-kawruh-link]'))) {
    const href = node.dataset.kawruhLink ?? ''
    let meta: LinkMeta = {}
    try {
      meta = JSON.parse(page.links[href] ?? '{}') as LinkMeta
    } catch {
      // Keep the plain title + domain card.
    }
    const view = node.dataset.view === 'preview' ? 'preview' : 'card'
    const wrap = el('div', 'kw-link')
    wrap.dataset.view = view
    const card = el('a', 'kw-link-card')
    fillLinkCard(card, { href, label: node.dataset.label ?? '', view, meta })
    wrap.append(card)
    node.replaceWith(wrap)
  }
  return out
}

/// Builds the print layout for `pages` (first one is the exported page) and
/// waits until its images are ready. Remove the returned element afterwards.
export async function preparePrint(pages: PrintPage[]): Promise<HTMLElement> {
  const root = el('div', 'kw-print')
  const byDepth: string[] = []
  for (const page of pages) {
    byDepth[page.depth] = page.title || 'Untitled'
    const section = el('section', 'kw-print-page')
    const header = el('header', 'kw-print-header')
    if (page.depth > 0) {
      header.append(el('div', 'kw-print-path', byDepth.slice(0, page.depth).join(' › ')))
    }
    const title = [page.icon, page.title || 'Untitled'].filter(Boolean).join(' ')
    header.append(el('h1', 'kw-print-title', title))
    section.append(header)
    if (page.depth === 0 && pages.length > 1) {
      // A short contents list when sub-pages follow.
      const toc = el('nav', 'kw-print-toc')
      toc.append(el('div', 'kw-print-toc-label', 'Contents'))
      for (const sub of pages.slice(1)) {
        const item = el('div', 'kw-print-toc-item', sub.title || 'Untitled')
        item.style.paddingLeft = `${(sub.depth - 1) * 16}px`
        toc.append(item)
      }
      section.append(toc)
    }
    section.append(await body(page))
    root.append(section)
  }
  document.body.append(root)
  const images = Array.from(root.querySelectorAll('img'))
  await Promise.race([
    Promise.all(images.map((i) => i.decode().catch(() => undefined))),
    new Promise((resolve) => setTimeout(resolve, 10_000)),
  ])
  return root
}
