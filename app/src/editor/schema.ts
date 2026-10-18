import { codeBlockSchema } from '@milkdown/kit/preset/commonmark'
import { $nodeSchema } from '@milkdown/kit/utils'

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

export const kawruhSchema = [codeBlockWithoutCanvas, canvasSchema].flat()
