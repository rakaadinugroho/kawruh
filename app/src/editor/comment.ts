/// Highlights the text a comment is about.
///
/// The anchor is stored as an encoded Yjs relative position, so it survives
/// anyone typing around it. Resolving it needs the collab binding, which is why
/// the plugin takes the page's Y.Doc and reads the mapping from the sync plugin.
import { $prose } from '@milkdown/kit/utils'
import { Plugin, PluginKey } from '@milkdown/kit/prose/state'
import type { EditorState } from '@milkdown/kit/prose/state'
import { Decoration, DecorationSet } from '@milkdown/kit/prose/view'
import type { EditorView } from '@milkdown/kit/prose/view'
import {
  absolutePositionToRelativePosition,
  relativePositionToAbsolutePosition,
  ySyncPluginKey,
} from 'y-prosemirror'
import type { ProsemirrorBinding } from 'y-prosemirror'
import * as Y from 'yjs'
import { toB64, fromB64 } from '../lib/b64'
import { commentsMap, readComments } from '../lib/comments'

export const commentKey = new PluginKey('kawruh-comment')

/// Where a comment's quoted text sits in the document.
export type Anchor = { quote: string; anchor: string; anchorEnd: string }

const bindingOf = (state: EditorState): ProsemirrorBinding | null =>
  ySyncPluginKey.getState(state)?.binding ?? null

/// The position an encoded relative position points at, or null when the text
/// it marked is gone.
function resolve(state: EditorState, encoded: string): number | null {
  if (!encoded) return null
  const binding = bindingOf(state)
  if (!binding) return null
  try {
    return relativePositionToAbsolutePosition(
      binding.doc,
      binding.type,
      Y.decodeRelativePosition(fromB64(encoded)),
      binding.mapping
    )
  } catch {
    return null
  }
}

/// The anchor for the current selection, or null when nothing is selected or
/// the editor isn't bound to a document yet.
export function anchorOfSelection(view: EditorView): Anchor | null {
  const { from, to } = view.state.selection
  if (from === to) return null
  const binding = bindingOf(view.state)
  if (!binding) return null
  const quote = view.state.doc.textBetween(from, to, ' ').trim()
  if (!quote) return null
  const encode = (pos: number) =>
    toB64(
      Y.encodeRelativePosition(
        absolutePositionToRelativePosition(pos, binding.type, binding.mapping)
      )
    )
  return { quote, anchor: encode(from), anchorEnd: encode(to) }
}

/// Draws a soft highlight behind every anchored comment. Comments without an
/// anchor (about the page itself) are not drawn here.
export const commentAnchors = (doc: Y.Doc) =>
  $prose(
    () =>
      new Plugin({
        key: commentKey,
        state: {
          init: () => 0,
          // The comments live outside the ProseMirror fragment, so their edits
          // arrive without a transaction; a bump is how we learn to redraw.
          apply(tr, v) {
            return tr.getMeta(commentKey) === undefined ? v : v + 1
          },
        },
        props: {
          decorations: (state) => {
            const decos: Decoration[] = []
            for (const c of readComments(doc)) {
              if (!c.anchor || !c.anchor_end) continue
              const from = resolve(state, c.anchor)
              const to = resolve(state, c.anchor_end)
              if (from === null || to === null || to <= from) continue
              decos.push(
                Decoration.inline(from, to, {
                  class: c.resolved ? 'kw-comment-done' : 'kw-comment-mark',
                  'data-comment': c.id,
                })
              )
            }
            return DecorationSet.create(state.doc, decos)
          },
        },
        view: (view) => {
          const bump = () => view.dispatch(view.state.tr.setMeta(commentKey, Date.now()))
          const map = commentsMap(doc)
          map.observe(bump)
          return { destroy: () => map.unobserve(bump) }
        },
      })
  )