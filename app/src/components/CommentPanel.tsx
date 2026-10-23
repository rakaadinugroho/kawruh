/// The comments sidebar: threads on the page, and a box to start one.
import { Check, CornerDownRight, MessageSquare, Trash2, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import type { EditorView } from '@milkdown/kit/prose/view'
import { type Anchor, anchorOfSelection } from '../editor/comment'
import type { DocHandle } from '../lib/docs'
import {
  type Comment,
  commentThreads,
  commentsMap,
  mention,
  newComment,
  removeComment,
  replyComment,
  segments,
  writeComment,
} from '../lib/comments'
import { type Member, api } from '../lib/ipc'
import { commentsKey } from '../lib/platform'
import { colorFor } from '../lib/hooks'
import { Button, IconButton } from './ui'

type Author = { id: string; name: string }

const ago = (ms: number) => {
  const s = Math.max(1, Math.round((Date.now() - ms) / 1000))
  if (s < 60) return 'just now'
  const m = Math.round(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.round(m / 60)
  if (h < 24) return `${h}h ago`
  return `${Math.round(h / 24)}d ago`
}

/// A comment body, with its mentions styled instead of shown as markdown.
export function CommentBody({ text, className = '' }: { text: string; className?: string }) {
  return (
    <p className={`whitespace-pre-wrap break-words ${className}`}>
      {segments(text).map((seg, i) =>
        seg.userId ? (
          <span key={i} className="rounded-[3px] bg-accent-soft px-1 text-accent">
            {seg.text}
          </span>
        ) : (
          <span key={i}>{seg.text}</span>
        )
      )}
    </p>
  )
}

function Avatar({ author, name }: { author: string; name: string }) {
  return (
    <span
      className="flex size-5 shrink-0 items-center justify-center rounded-full text-[9px] font-semibold text-white"
      style={{ background: colorFor(author || name) }}
      aria-hidden
    >
      {(name.trim()[0] ?? '?').toUpperCase()}
    </span>
  )
}

/// A textarea with `@` picking a person out of the workspace.
function Composer({
  placeholder,
  members,
  onSubmit,
  autoFocus,
  onCancel,
}: {
  placeholder: string
  members: Member[]
  onSubmit: (body: string) => void
  autoFocus?: boolean
  onCancel?: () => void
}) {
  const [text, setText] = useState('')
  const [query, setQuery] = useState<string | null>(null)
  const ref = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    if (autoFocus) ref.current?.focus()
  }, [autoFocus])

  // The picker shows while the text before the caret ends in `@` or `@word`.
  const sync = () => {
    const el = ref.current
    if (!el) return
    const upTo = el.value.slice(0, el.selectionStart ?? 0)
    const match = /(^|\s)@([\w.-]*)$/.exec(upTo)
    setQuery(match ? match[2] : null)
  }

  const matches = useMemo(() => {
    if (query === null) return []
    const q = query.toLowerCase()
    return members
      .filter((m) => `${m.displayName} ${m.username}`.toLowerCase().includes(q))
      .slice(0, 6)
  }, [query, members])

  const pick = (m: Member) => {
    const el = ref.current
    if (!el) return
    const at = el.selectionStart ?? 0
    const before = el.value.slice(0, at).replace(/@[\w.-]*$/, '')
    const after = el.value.slice(at)
    const next = `${before}${mention(m.userId, m.displayName || m.username)} ${after}`
    setText(next)
    setQuery(null)
    requestAnimationFrame(() => {
      const caret = before.length + mention(m.userId, m.displayName || m.username).length
      el.focus()
      el.setSelectionRange(caret, caret)
    })
  }

  const submit = () => {
    const body = text.trim()
    if (!body) return
    onSubmit(body)
    setText('')
    setQuery(null)
  }

  return (
    <div className="relative">
      <textarea
        ref={ref}
        value={text}
        rows={2}
        placeholder={placeholder}
        aria-label={placeholder}
        data-testid="comment-input"
        onChange={(e) => {
          setText(e.target.value)
          sync()
        }}
        onKeyUp={sync}
        onClick={sync}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.stopPropagation()
            onCancel?.()
          } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
            e.preventDefault()
            submit()
          }
        }}
        className="w-full resize-none rounded-md bg-paper px-2.5 py-2 text-[13px] leading-[1.5] text-ink shadow-[inset_0_0_0_1px_var(--line)] placeholder:text-ink-3 focus:shadow-[inset_0_0_0_1.5px_var(--accent)] focus:outline-none"
      />
      {query !== null && matches.length > 0 && (
        <div className="kw-menu absolute right-0 bottom-full z-10 mb-1 w-full rounded-md bg-raised p-1 shadow-pop">
          {matches.map((m) => (
            <button
              key={m.userId}
              type="button"
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => pick(m)}
              className="press flex h-7 w-full items-center gap-2 rounded-[4px] px-1.5 text-left text-[13px] text-ink hover:bg-hover"
            >
              <Avatar author={m.userId} name={m.displayName || m.username} />
              <span className="truncate">{m.displayName || m.username}</span>
              <span className="ml-auto truncate text-[11px] text-ink-3">@{m.username}</span>
            </button>
          ))}
        </div>
      )}
      <div className="mt-1.5 flex items-center justify-end gap-1.5">
        {onCancel && (
          <Button variant="ghost" className="h-7 px-2 text-[12px]" onClick={onCancel}>
            Cancel
          </Button>
        )}
        <Button
          variant="primary"
          className="h-7 px-2.5 text-[12px]"
          disabled={!text.trim()}
          onClick={submit}
          data-testid="comment-send"
        >
          Comment
        </Button>
      </div>
    </div>
  )
}

export function CommentPanel({
  handle,
  workspaceId,
  readonly,
  author,
  editorView,
  focusCommentId,
  onClose,
  onPageId,
}: {
  handle: DocHandle
  workspaceId: string
  readonly: boolean
  author: Author | null
  /// The page's editor view, so a comment can be about the current selection.
  editorView: EditorView | null
  /// A thread to scroll to and highlight (from the inbox).
  focusCommentId?: string | null
  onClose: () => void
  onPageId?: (id: string) => void
}) {
  const [version, setVersion] = useState(0)
  const [members, setMembers] = useState<Member[]>([])
  const [replyTo, setReplyTo] = useState<string | null>(null)
  const [showResolved, setShowResolved] = useState(false)
  const scroll = useRef<HTMLDivElement>(null)

  // Comments live in the document, so anything written anywhere shows up here.
  useEffect(() => {
    const bump = () => setVersion((v) => v + 1)
    const map = commentsMap(handle.doc)
    map.observe(bump)
    handle.ready.then(bump).catch(() => {})
    return () => map.unobserve(bump)
  }, [handle])

  useEffect(() => {
    let stale = false
    api
      .members(workspaceId)
      .then((m) => !stale && setMembers(m))
      .catch(() => {})
    return () => {
      stale = true
    }
  }, [workspaceId])

  const threads = useMemo(
    () => commentThreads(handle.doc).filter((t) => showResolved || !t[0].resolved),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [handle, version, showResolved]
  )
  const hidden = useMemo(
    () => commentThreads(handle.doc).filter((t) => t[0].resolved).length,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [handle, version]
  )
  /// The root of the thread holding `focusCommentId` (a reply highlights its thread).
  const focusedId = useMemo(() => {
    if (!focusCommentId) return null
    const hit = commentThreads(handle.doc).find((t) => t.some((c) => c.id === focusCommentId))
    return hit ? hit[0].id : null
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [handle, version, focusCommentId])

  // What the editor has selected right now, if anything.
  const [anchor, setAnchor] = useState<Anchor | null>(null)
  useEffect(() => {
    if (!editorView) return setAnchor(null)
    const read = () => setAnchor(anchorOfSelection(editorView))
    read()
    editorView.dom.addEventListener('mouseup', read)
    editorView.dom.addEventListener('keyup', read)
    return () => {
      editorView.dom.removeEventListener('mouseup', read)
      editorView.dom.removeEventListener('keyup', read)
    }
  }, [editorView, version])

  const post = (body: string) => {
    if (!author) return
    if (replyTo) {
      const parent = commentThreads(handle.doc)
        .flat()
        .find((c) => c.id === replyTo)
      if (parent) writeComment(handle.doc, replyComment(body, author, parent))
      setReplyTo(null)
      return
    }
    writeComment(
      handle.doc,
      newComment(body, author, {
        quote: anchor?.quote,
        anchor: anchor?.anchor,
        anchorEnd: anchor?.anchorEnd,
      })
    )
    setAnchor(null)
  }

  const patch = (c: Comment, changes: Partial<Comment>) =>
    writeComment(handle.doc, { ...c, ...changes })

  // A thread opened from the inbox scrolls itself into view.
  useEffect(() => {
    if (!focusCommentId) return
    const el = scroll.current?.querySelector<HTMLElement>('.kw-comment-focus')
    el?.scrollIntoView({ block: 'center' })
  }, [focusCommentId, version, threads.length])

  return (
    <aside
      className="flex h-full w-[320px] shrink-0 flex-col border-l border-line bg-sidebar"
      data-testid="comment-panel"
      aria-label="Comments"
    >
      <div className="flex h-11 shrink-0 items-center justify-between pr-2 pl-4">
        <h2 className="flex items-center gap-2 text-[13px] font-semibold text-ink">
          <MessageSquare size={14} className="text-ink-3" />
          Comments
          {threads.length > 0 && <span className="text-ink-3 tabular-nums">{threads.length}</span>}
        </h2>
        <IconButton label={`Hide comments (${commentsKey})`} onClick={onClose}>
          <X size={15} />
        </IconButton>
      </div>

      <div ref={scroll} className="min-h-0 flex-1 overflow-y-auto px-4 pb-3">
        {threads.length === 0 ? (
          <p className="pt-6 text-center text-[13px] text-ink-3">
            No comments yet. Select some text, or just start talking about the page.
          </p>
        ) : (
          <ul className="flex flex-col gap-4">
            {threads.map((thread) => {
              const root = thread[0]
              const focused = focusedId !== null && root.id === focusedId
              return (
                <li
                  key={root.id}
                  data-testid="comment-thread"
                  className={`rounded-lg px-1 pb-3 ${
                    focused ? 'kw-comment-focus bg-accent-soft' : root.resolved ? 'opacity-60' : ''
                  }`}
                >
                  {root.quote && (
                    <blockquote className="mb-1.5 border-l-2 border-amber pl-2 text-[12px] text-ink-3 italic">
                      {root.quote}
                    </blockquote>
                  )}
                  <ul className="flex flex-col gap-3">
                    {thread.map((c) => (
                      <li key={c.id} className="flex gap-2">
                        <Avatar author={c.author} name={c.author_name || '?'} />
                        <div className="min-w-0 flex-1">
                          <div className="flex items-baseline gap-1.5 text-[12px]">
                            <span className="font-semibold text-ink">
                              {c.author === author?.id ? 'You' : c.author_name || 'Someone'}
                            </span>
                            <span className="text-ink-3">{ago(c.created)}</span>
                          </div>
                          <div className="text-[13px] leading-[1.5] text-ink-2">
                            <CommentBody text={c.body} />
                          </div>
                        </div>
                      </li>
                    ))}
                  </ul>
                  {!readonly && (
                    <div className="mt-2 flex items-center gap-1 pl-7">
                      <button
                        type="button"
                        onClick={() => setReplyTo(replyTo === root.id ? null : root.id)}
                        className="press flex items-center gap-1 rounded-[4px] px-1.5 py-0.5 text-[12px] text-ink-3 hover:bg-hover hover:text-ink"
                      >
                        <CornerDownRight size={12} />
                        Reply
                      </button>
                      <button
                        type="button"
                        onClick={() => patch(root, { resolved: !root.resolved })}
                        className="press flex items-center gap-1 rounded-[4px] px-1.5 py-0.5 text-[12px] text-ink-3 hover:bg-hover hover:text-ink"
                      >
                        <Check size={12} />
                        {root.resolved ? 'Reopen' : 'Resolve'}
                      </button>
                      <button
                        type="button"
                        aria-label="Delete thread"
                        onClick={() => thread.forEach((c) => removeComment(handle.doc, c.id))}
                        className="press ml-auto rounded-[4px] p-1 text-ink-3 hover:bg-hover hover:text-danger"
                      >
                        <Trash2 size={12} />
                      </button>
                    </div>
                  )}
                  {replyTo === root.id && (
                    <div className="mt-2 pl-7">
                      <Composer
                        autoFocus
                        placeholder="Reply…"
                        members={members}
                        onCancel={() => setReplyTo(null)}
                        onSubmit={post}
                      />
                    </div>
                  )}
                </li>
              )
            })}
          </ul>
        )}
        {hidden > 0 && (
          <button
            type="button"
            onClick={() => setShowResolved((v) => !v)}
            className="press mt-3 w-full rounded-md py-1 text-[12px] text-ink-3 hover:bg-hover"
          >
            {showResolved ? 'Hide' : 'Show'} {hidden} resolved {hidden === 1 ? 'thread' : 'threads'}
          </button>
        )}
      </div>

      {!readonly && author && (
        <div className="shrink-0 border-t border-line px-4 py-3">
          {anchor && (
            <p className="mb-1.5 truncate border-l-2 border-amber pl-2 text-[12px] text-ink-3 italic">
              “{anchor.quote}”
              <button
                type="button"
                onClick={() => setAnchor(null)}
                className="press ml-1.5 text-ink-3 hover:text-ink"
              >
                clear
              </button>
            </p>
          )}
          <Composer
            placeholder={anchor ? 'Comment on this…' : 'Add a comment…'}
            members={members}
            onSubmit={post}
          />
        </div>
      )}
      {readonly && onPageId && (
        <p className="shrink-0 border-t border-line px-4 py-3 text-[12px] text-ink-3">
          You have read-only access, so you can read comments but not write them.
        </p>
      )}
    </aside>
  )
}
