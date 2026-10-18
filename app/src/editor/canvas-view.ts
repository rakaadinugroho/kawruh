/// Node view for `kawruh_canvas` blocks: shows the stored SVG preview and
/// opens the full canvas editor. Scene data lives in the page doc
/// (`canvas:<id>` map), previews in `canvas_svg`.
import { $view } from '@milkdown/kit/utils'
import type * as Y from 'yjs'
import { canvasSchema } from './schema'

export const canvasPreviews = (doc: Y.Doc) => doc.getMap<string>('canvas_svg')

export function canvasView(doc: Y.Doc, open: (id: string) => void) {
  return $view(canvasSchema.node, () => (node) => {
    let id = String(node.attrs.id ?? '')
    const previews = canvasPreviews(doc)

    const dom = document.createElement('div')
    dom.className = 'kw-canvas'
    dom.contentEditable = 'false'
    dom.dataset.kawruhCanvas = id

    const preview = document.createElement('div')
    preview.className = 'kw-canvas-preview'

    const bar = document.createElement('div')
    bar.className = 'kw-canvas-bar'
    const label = document.createElement('span')
    label.textContent = 'Canvas'
    const button = document.createElement('button')
    button.type = 'button'
    button.textContent = 'Open canvas'
    bar.append(label, button)
    dom.append(preview, bar)

    const render = () => {
      const svg = previews.get(id)
      preview.replaceChildren()
      if (svg) {
        const img = new Image()
        img.alt = 'Canvas drawing'
        img.draggable = false
        img.src = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`
        preview.append(img)
      } else {
        const empty = document.createElement('span')
        empty.className = 'kw-canvas-empty'
        empty.textContent = 'Empty canvas. Open it to start drawing.'
        preview.append(empty)
      }
    }
    const onPreview = (e: Y.YMapEvent<string>) => {
      if (e.keysChanged.has(id)) render()
    }
    previews.observe(onPreview)
    render()

    const openIt = (e: Event) => {
      e.preventDefault()
      e.stopPropagation()
      open(id)
    }
    button.addEventListener('click', openIt)
    preview.addEventListener('dblclick', openIt)

    return {
      dom,
      update: (next) => {
        if (next.type !== node.type) return false
        const nextId = String(next.attrs.id ?? '')
        if (nextId !== id) {
          id = nextId
          dom.dataset.kawruhCanvas = id
          render()
        }
        return true
      },
      stopEvent: (e) => e.target instanceof HTMLButtonElement,
      ignoreMutation: () => true,
      destroy: () => previews.unobserve(onPreview),
    }
  })
}
