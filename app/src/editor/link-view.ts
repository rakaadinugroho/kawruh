/// Node view for `kawruh_link`: a bookmark card (text left, thumbnail right)
/// or a large preview (image on top). Clicking it opens the link in the browser.
import { linkSchema } from '@milkdown/kit/preset/commonmark'
import type { Node } from '@milkdown/kit/prose/model'
import { $view } from '@milkdown/kit/utils'
import { openUrl } from '@tauri-apps/plugin-opener'
import type * as Y from 'yjs'
import type { LinkMeta } from '../lib/ipc'
import { ensureMeta, hostOf, linkMetaMap, readMeta } from './link-meta'
import { type LinkView, linkCardSchema } from './schema'

function el<K extends keyof HTMLElementTagNameMap>(tag: K, className: string, text?: string) {
  const e = document.createElement(tag)
  e.className = className
  if (text) e.textContent = text
  return e
}

function img(src: string, className: string) {
  const i = el('img', className)
  i.alt = ''
  i.draggable = false
  i.referrerPolicy = 'no-referrer'
  i.addEventListener('error', () => i.remove())
  i.src = src
  return i
}

export function linkCardView(doc: Y.Doc, canWrite: () => boolean) {
  return $view(linkCardSchema.node, (ctx) => (initial, view, getPos) => {
    let node: Node = initial
    const dom = el('div', 'kw-link')
    dom.contentEditable = 'false'

    const card = el('a', 'kw-link-card')
    card.draggable = false
    const tools = el('div', 'kw-link-tools')
    dom.append(card, tools)

    const href = () => String(node.attrs.href ?? '')

    const setAttrs = (attrs: Record<string, unknown>) => {
      const pos = getPos()
      if (pos === undefined || !canWrite()) return
      view.dispatch(view.state.tr.setNodeMarkup(pos, undefined, { ...node.attrs, ...attrs }))
    }

    /// Back to an ordinary link in a paragraph.
    const toLink = () => {
      const pos = getPos()
      if (pos === undefined || !canWrite()) return
      const { schema } = view.state
      const mark = linkSchema.type(ctx).create({ href: href() })
      const text = schema.text(String(node.attrs.label || href()), [mark])
      const para = schema.nodes.paragraph.create(null, text)
      view.dispatch(view.state.tr.replaceWith(pos, pos + node.nodeSize, para))
    }

    const button = (label: string, onClick: () => void, pressed = false) => {
      const b = el('button', 'kw-link-tool', label)
      b.type = 'button'
      if (pressed) b.setAttribute('aria-pressed', 'true')
      b.addEventListener('mousedown', (e) => e.preventDefault())
      b.addEventListener('click', (e) => {
        e.preventDefault()
        e.stopPropagation()
        onClick()
      })
      return b
    }

    const render = () => {
      const kind = (node.attrs.view === 'preview' ? 'preview' : 'card') as LinkView
      const meta: LinkMeta = readMeta(doc, href()) ?? {}
      const title = meta.title || String(node.attrs.label || '') || href()
      dom.dataset.view = kind
      card.href = href()
      card.title = href()

      const text = el('div', 'kw-link-text')
      text.append(el('div', 'kw-link-title', title))
      if (meta.description) text.append(el('div', 'kw-link-desc', meta.description))
      const site = el('div', 'kw-link-site')
      if (meta.icon) site.append(img(meta.icon, 'kw-link-icon'))
      site.append(el('span', '', meta.site || hostOf(href())))
      text.append(site)

      card.replaceChildren()
      if (kind === 'preview' && meta.image) card.append(img(meta.image, 'kw-link-hero'))
      card.append(text)
      if (kind === 'card' && meta.image) card.append(img(meta.image, 'kw-link-thumb'))

      tools.replaceChildren()
      if (canWrite()) {
        tools.append(
          button('Card', () => setAttrs({ view: 'card' }), kind === 'card'),
          button('Preview', () => setAttrs({ view: 'preview' }), kind === 'preview'),
          button('Link', toLink)
        )
      }
      tools.append(button('Open ↗', () => void openUrl(href())))
    }

    const fill = () => {
      if (readMeta(doc, href())) return
      void ensureMeta(doc, href(), canWrite())
    }

    const onMeta = (e: Y.YMapEvent<string>) => {
      if (!e.keysChanged.has(href())) return
      render()
      // A card made before its metadata arrived gets the page title as label,
      // so exported markdown reads well.
      const meta = readMeta(doc, href())
      if (meta?.title && !node.attrs.label) setAttrs({ label: meta.title })
    }
    linkMetaMap(doc).observe(onMeta)

    card.addEventListener('click', (e) => {
      e.preventDefault()
      void openUrl(href())
    })

    render()
    fill()

    return {
      dom,
      update: (next) => {
        if (next.type !== node.type) return false
        const changed = next.attrs.href !== node.attrs.href
        node = next
        render()
        if (changed) fill()
        return true
      },
      stopEvent: (e) => e.target instanceof HTMLElement && !!e.target.closest('.kw-link-tools'),
      ignoreMutation: () => true,
      destroy: () => linkMetaMap(doc).unobserve(onMeta),
    }
  })
}
