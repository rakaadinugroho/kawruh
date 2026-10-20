/// Links between pages. Typing `[[` opens a page picker; choosing a page
/// inserts `[Title](kawruh://page/<id>)`, an ordinary markdown link that the
/// app opens as a page.
import type { Ctx } from '@milkdown/kit/ctx'
import { linkSchema } from '@milkdown/kit/preset/commonmark'
import { Plugin, PluginKey } from '@milkdown/kit/prose/state'
import type { EditorView } from '@milkdown/kit/prose/view'
import { $prose } from '@milkdown/kit/utils'

export const PAGE_HREF = 'kawruh://page/'

/// The page id a link points at, or null for other links.
export function pageIdOf(href: string | null | undefined): string | null {
  return href?.startsWith(PAGE_HREF) ? href.slice(PAGE_HREF.length) || null : null
}

type Mention = { from: number; query: string } | null

const mentionKey = new PluginKey<Mention>('kawruh-page-mention')

export type MentionHooks = {
  /// The picker should show (with the text typed after `[[`) or hide (null).
  onChange: (m: { query: string; x: number; y: number } | null) => void
  /// Keys while the picker is open; return true when handled.
  onKey: (key: 'ArrowUp' | 'ArrowDown' | 'Enter' | 'Escape') => boolean
}

export const pageMention = (hooks: MentionHooks) =>
  $prose(
    () =>
      new Plugin<Mention>({
        key: mentionKey,
        state: {
          init: () => null,
          apply(tr, value, _old, state) {
            const meta = tr.getMeta(mentionKey)
            if (meta !== undefined) return meta
            if (!value) return null
            const from = tr.mapping.map(value.from)
            const head = state.selection.head
            if (!state.selection.empty || head < from + 2) return null
            const typed = state.doc.textBetween(from, head, '\n')
            if (!typed.startsWith('[[') || /[\n\]]/.test(typed.slice(2)) || typed.length > 80) return null
            return { from, query: typed.slice(2) }
          },
        },
        props: {
          handleTextInput(view, from, to, text) {
            if (text !== '[' || view.state.selection.$from.parent.type.spec.code) return false
            if (view.state.doc.textBetween(Math.max(0, from - 1), from) !== '[') return false
            view.dispatch(view.state.tr.insertText(text, from, to).setMeta(mentionKey, { from: from - 1, query: '' }))
            return true
          },
          handleKeyDown(view, event) {
            if (!mentionKey.getState(view.state)) return false
            const key = event.key
            if (key === 'ArrowUp' || key === 'ArrowDown' || key === 'Enter' || key === 'Escape') {
              const handled = hooks.onKey(key)
              if (handled && key === 'Escape') view.dispatch(view.state.tr.setMeta(mentionKey, null))
              return handled
            }
            return false
          },
        },
        view: () => ({
          update(view) {
            const m = mentionKey.getState(view.state)
            if (!m) return hooks.onChange(null)
            const at = view.coordsAtPos(m.from)
            hooks.onChange({ query: m.query, x: at.left, y: at.bottom + 6 })
          },
          destroy: () => hooks.onChange(null),
        }),
      })
  )

/// Starts a mention at the cursor, as if `[[` had been typed (slash menu).
export function startMention(view: EditorView) {
  const { from } = view.state.selection
  view.dispatch(view.state.tr.insertText('[[').setMeta(mentionKey, { from, query: '' }))
  view.focus()
}

/// Replaces the typed `[[query` with a link to the chosen page.
export function insertPageLink(view: EditorView, ctx: Ctx, page: { id: string; title: string }) {
  const m = mentionKey.getState(view.state)
  if (!m) return
  const mark = linkSchema.type(ctx).create({ href: PAGE_HREF + page.id })
  const head = view.state.selection.head
  const tr = view.state.tr
    .replaceWith(m.from, head, view.state.schema.text(page.title || 'Untitled', [mark]))
    .removeStoredMark(mark.type)
    .setMeta(mentionKey, null)
  view.dispatch(tr)
  view.focus()
}
