/// The ProseMirror half of Assist: where a selection's blocks start and end,
/// how the text selected reads as markdown, and how an agent's markdown lands
/// back in the page. Every change the user applies is one transaction (one
/// undo step), and nothing is touched before they press Apply.
import { parserCtx, serializerCtx } from '@milkdown/kit/core'
import type { Ctx } from '@milkdown/kit/ctx'
import { type Node, Slice } from '@milkdown/kit/prose/model'
import type { EditorView } from '@milkdown/kit/prose/view'

/// Expands a selection to the whole top-level blocks it touches, so a rewrite
/// never half-replaces a paragraph or list item.
export function blockRange(view: EditorView, from: number, to: number): [number, number] {
  const doc = view.state.doc
  const $from = doc.resolve(from)
  const $to = doc.resolve(to)
  const a = $from.depth === 0 ? $from.pos : $from.before(1)
  const b = $to.depth === 0 ? $to.pos : $to.after(1)
  return a <= b ? [a, b] : [b, a]
}

/// The range as markdown (both the prompts and the preview speak markdown).
export function markdownInRange(
  view: EditorView,
  ctx: Ctx,
  from: number,
  to: number,
): string {
  return ctx.get(serializerCtx)(view.state.doc.cut(from, to)) ?? ''
}

function parse(ctx: Ctx, markdown: string): Node | null {
  if (!markdown.trim()) return null
  try {
    return ctx.get(parserCtx)(markdown) ?? null
  } catch {
    return null
  }
}

export type ApplyResult = 'applied' | 'gone' | 'empty'

/// Replaces the blocks of `range` with the agent's markdown. `anchor` is what
/// the selection read when the agent started: a page edited meanwhile (by the
/// user, a collaborator, an MCP agent) is never clobbered.
export function applyRewrite(
  view: EditorView,
  ctx: Ctx,
  range: readonly [number, number],
  anchor: string,
  markdown: string,
): ApplyResult {
  const [from, to] = range
  const now = view.state.doc.textBetween(from, to, '\n', ' ').trim()
  if (now !== anchor.trim()) return 'gone'
  const parsed = parse(ctx, markdown)
  if (!parsed || parsed.content.size === 0) return 'empty'
  view.dispatch(
    view.state.tr.replaceRange(from, to, new Slice(parsed.content, 0, 0)).scrollIntoView(),
  )
  view.focus()
  return 'applied'
}

/// Lands a draft at the cursor's block: replacing it (the slash-typed "/" text
/// or an empty block) or inserting after it (writing mid-paragraph).
export function applyDraft(
  view: EditorView,
  ctx: Ctx,
  range: readonly [number, number],
  replaceBlock: boolean,
  markdown: string,
): ApplyResult {
  const [from, to] = range
  const parsed = parse(ctx, markdown)
  if (!parsed || parsed.content.size === 0) return 'empty'
  const at = replaceBlock ? from : to
  view.dispatch(
    view.state.tr.replaceRange(at, at, new Slice(parsed.content, 0, 0)).scrollIntoView(),
  )
  view.focus()
  return 'applied'
}
