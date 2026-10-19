/// Pasting a lone URL makes it a link right away and offers to show it as a
/// titled link, a card or a large preview instead (the "Paste as" menu).
import type { Ctx } from '@milkdown/kit/ctx'
import { linkSchema } from '@milkdown/kit/preset/commonmark'
import { Plugin, PluginKey } from '@milkdown/kit/prose/state'
import type { EditorView } from '@milkdown/kit/prose/view'
import { $prose } from '@milkdown/kit/utils'
import { ySyncPluginKey } from 'y-prosemirror'
import { type LinkView, linkCardSchema } from './schema'

const URL_RE = /^https?:\/\/[^\s<>"]+$/i

type Pasted = { from: number; to: number; href: string }

export const pastedLinkKey = new PluginKey<Pasted | null>('kawruh-pasted-link')

export type PasteLinkHooks = {
  /// A URL was pasted as a link; `x`/`y` is where to show the menu.
  onPaste: (p: { href: string; x: number; y: number }) => void
  /// The pasted link was edited or removed; the menu should go.
  onDone: () => void
}

export const pasteLink = (hooks: PasteLinkHooks) =>
  $prose((ctx) => {
    return new Plugin<Pasted | null>({
      key: pastedLinkKey,
      state: {
        init: () => null,
        apply(tr, value) {
          const meta = tr.getMeta(pastedLinkKey)
          if (meta !== undefined) return meta
          if (!value || !tr.docChanged) return value
          // A collaborator's edit only shifts the link; your own edit settles it.
          if (!tr.getMeta(ySyncPluginKey)) return null
          const from = tr.mapping.map(value.from, 1)
          const to = tr.mapping.map(value.to, -1)
          return from < to ? { ...value, from, to } : null
        },
      },
      view: () => ({
        update(view, prev) {
          if (pastedLinkKey.getState(prev) && !pastedLinkKey.getState(view.state)) hooks.onDone()
        },
      }),
      props: {
        handleDOMEvents: {
          paste(view, event) {
            const data = event.clipboardData
            if (!data || data.files.length > 0) return false
            const href = data.getData('text/plain').trim()
            if (!URL_RE.test(href)) return false
            const { selection, schema } = view.state
            if (selection.$from.parent.type.spec.code) return false
            event.preventDefault()
            const mark = linkSchema.type(ctx).create({ href })
            if (!selection.empty) {
              // Pasting over a selection links it.
              view.dispatch(view.state.tr.addMark(selection.from, selection.to, mark))
              return true
            }
            const from = selection.from
            const tr = view.state.tr
              .replaceSelectionWith(schema.text(href, [mark]), false)
              .removeStoredMark(mark.type)
              .setMeta(pastedLinkKey, { from, to: from + href.length, href })
            view.dispatch(tr)
            const end = view.coordsAtPos(from + href.length)
            hooks.onPaste({ href, x: end.left, y: end.bottom + 6 })
            return true
          },
        },
      },
    })
  })

function pasted(view: EditorView): Pasted | null {
  const p = pastedLinkKey.getState(view.state)
  if (!p || view.state.doc.textBetween(p.from, p.to) !== p.href) return null
  return p
}

/// Leaves the pasted URL as it is.
export function keepPastedLink(view: EditorView) {
  if (pastedLinkKey.getState(view.state)) view.dispatch(view.state.tr.setMeta(pastedLinkKey, null))
}

/// Swaps the pasted URL's text for the page title, keeping the link.
export function titlePastedLink(view: EditorView, title: string) {
  const p = pasted(view)
  if (!p || !title) return keepPastedLink(view)
  const marks = view.state.doc.resolve(p.from + 1).marks()
  const tr = view.state.tr
    .replaceWith(p.from, p.to, view.state.schema.text(title, marks))
    .setMeta(pastedLinkKey, null)
  view.dispatch(tr)
}

/// Turns the pasted URL into a card or preview block.
export function blockPastedLink(view: EditorView, ctx: Ctx, kind: LinkView, label: string) {
  const p = pasted(view)
  if (!p) return keepPastedLink(view)
  const node = linkCardSchema.type(ctx).create({ href: p.href, label, view: kind })
  const tr = view.state.tr.delete(p.from, p.to)
  const $pos = tr.doc.resolve(p.from)
  if ($pos.parent.type.name === 'paragraph' && $pos.parent.content.size === 0) {
    tr.replaceWith($pos.before(), $pos.after(), node)
  } else {
    tr.replaceRangeWith(p.from, p.from, node)
  }
  view.dispatch(tr.setMeta(pastedLinkKey, null).scrollIntoView())
}
