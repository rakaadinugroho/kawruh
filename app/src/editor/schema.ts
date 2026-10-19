import { codeBlockSchema, imageSchema, paragraphSchema } from '@milkdown/kit/preset/commonmark'
import type { MarkdownNode } from '@milkdown/kit/transformer'
import { $nodeSchema } from '@milkdown/kit/utils'
import { canonicalSrc, displaySrc } from './assets'

/// Fenced-code language used to persist a canvas block in markdown:
/// ```kawruh-canvas\n<canvas id>\n```
export const CANVAS_LANG = 'kawruh-canvas'
export const CANVAS_NODE = 'kawruh_canvas'

export const canvasSchema = $nodeSchema(CANVAS_NODE, () => ({
  group: 'block',
  atom: true,
  isolating: true,
  selectable: true,
  draggable: true,
  marks: '',
  attrs: {
    id: { default: '', validate: 'string' },
  },
  parseDOM: [
    {
      tag: 'div[data-kawruh-canvas]',
      getAttrs: (dom) => ({
        id: (dom as HTMLElement).dataset.kawruhCanvas ?? '',
      }),
    },
  ],
  toDOM: (node) => ['div', { 'data-kawruh-canvas': node.attrs.id }],
  parseMarkdown: {
    match: (node) => node.type === 'code' && node.lang === CANVAS_LANG,
    runner: (state, node, type) => {
      state.addNode(type, { id: String(node.value ?? '').trim() })
    },
  },
  toMarkdown: {
    match: (node) => node.type.name === CANVAS_NODE,
    runner: (state, node) => {
      state.addNode('code', undefined, node.attrs.id, { lang: CANVAS_LANG })
    },
  },
}))

/// The commonmark code block would otherwise claim canvas fences first,
/// because the parser picks the first schema node whose matcher accepts.
export const codeBlockWithoutCanvas = codeBlockSchema.extendSchema(
  (prev) => (ctx) => {
    const base = prev(ctx)
    return {
      ...base,
      parseMarkdown: {
        ...base.parseMarkdown,
        match: (node) => node.type === 'code' && node.lang !== CANVAS_LANG,
      },
    }
  }
)

/// A link shown as a card or a large preview. In markdown it is a paragraph
/// holding only `[label](href "kawruh:card")` (or `"kawruh:preview"`), so any
/// other markdown tool still sees a plain link.
export const LINK_NODE = 'kawruh_link'
export const LINK_VIEWS = ['card', 'preview'] as const
export type LinkView = (typeof LINK_VIEWS)[number]
const LINK_TITLE_PREFIX = 'kawruh:'

function cardLink(node: MarkdownNode): { link: MarkdownNode; view: LinkView } | null {
  if (node.type !== 'paragraph' || node.children?.length !== 1) return null
  const link = node.children[0]
  if (link.type !== 'link' || typeof link.title !== 'string') return null
  const view = link.title.slice(LINK_TITLE_PREFIX.length) as LinkView
  if (!link.title.startsWith(LINK_TITLE_PREFIX) || !LINK_VIEWS.includes(view)) return null
  return { link, view }
}

function plainText(node: MarkdownNode): string {
  if (typeof node.value === 'string') return node.value
  return (node.children ?? []).map(plainText).join('')
}

export const linkCardSchema = $nodeSchema(LINK_NODE, () => ({
  group: 'block',
  atom: true,
  isolating: true,
  selectable: true,
  draggable: true,
  marks: '',
  attrs: {
    href: { default: '', validate: 'string' },
    label: { default: '', validate: 'string' },
    view: { default: 'card', validate: 'string' },
  },
  parseDOM: [
    {
      tag: 'div[data-kawruh-link]',
      getAttrs: (dom) => {
        const el = dom as HTMLElement
        return {
          href: el.dataset.kawruhLink ?? '',
          label: el.dataset.label ?? '',
          view: el.dataset.view === 'preview' ? 'preview' : 'card',
        }
      },
    },
  ],
  toDOM: (node) => [
    'div',
    { 'data-kawruh-link': node.attrs.href, 'data-label': node.attrs.label, 'data-view': node.attrs.view },
  ],
  parseMarkdown: {
    match: (node) => cardLink(node) !== null,
    runner: (state, node, type) => {
      const { link, view } = cardLink(node)!
      state.addNode(type, { href: String(link.url ?? ''), label: plainText(link), view })
    },
  },
  toMarkdown: {
    match: (node) => node.type.name === LINK_NODE,
    runner: (state, node) => {
      const view = LINK_VIEWS.includes(node.attrs.view) ? node.attrs.view : 'card'
      state.openNode('paragraph')
      state.openNode('link', undefined, { url: node.attrs.href, title: LINK_TITLE_PREFIX + view })
      if (node.attrs.label) state.addNode('text', undefined, node.attrs.label)
      state.closeNode()
      state.closeNode()
    },
  },
}))

/// Paragraphs that are link cards belong to `kawruh_link` (the first matching
/// schema node wins, and paragraph is registered earlier).
export const paragraphWithoutCards = paragraphSchema.extendSchema((prev) => (ctx) => {
  const base = prev(ctx)
  return {
    ...base,
    parseMarkdown: {
      ...base.parseMarkdown,
      match: (node) => node.type === 'paragraph' && cardLink(node) === null,
    },
  }
})

/// Images keep `kawruh-asset://` in the document and load through the protocol.
export const imageWithAssets = imageSchema.extendSchema((prev) => (ctx) => {
  const base = prev(ctx)
  return {
    ...base,
    parseDOM: [
      {
        tag: 'img[src]',
        getAttrs: (dom) => {
          const el = dom as HTMLElement
          return {
            src: canonicalSrc(el.getAttribute('src') || ''),
            alt: el.getAttribute('alt') || '',
            title: el.getAttribute('title') || '',
          }
        },
      },
    ],
    toDOM: (node) => {
      const [tag, attrs] = base.toDOM!(node) as [string, Record<string, unknown>]
      return [tag, { ...attrs, src: displaySrc(String(node.attrs.src ?? '')) }]
    },
  }
})

export const kawruhSchema = [
  codeBlockWithoutCanvas,
  canvasSchema,
  paragraphWithoutCards,
  linkCardSchema,
  imageWithAssets,
].flat()
