import { CornerDownLeft, FileText, SquarePen, TextSearch } from 'lucide-react'
import { Fragment, useEffect, useMemo, useRef, useState } from 'react'
import type * as Y from 'yjs'
import { type SearchHit, api } from '../lib/ipc'
import { type Page, createPage, pagePath } from '../lib/workspace'

type Result =
  | { kind: 'page'; page: Page; path: string }
  | { kind: 'text'; page: Page; snippet: string }
  | { kind: 'create'; title: string }

/// `text` with every query word wrapped in <mark>.
function Highlight({ text, query }: { text: string; query: string }) {
  const words = query.trim().split(/\s+/).filter(Boolean)
  if (!words.length) return <>{text}</>
  const escaped = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'))
  const parts = text.split(new RegExp(`(${escaped.join('|')})`, 'gi'))
  return (
    <>
      {parts.map((part, i) =>
        i % 2 ? (
          <mark key={i} className="rounded-[2px] bg-accent-soft text-ink">
            {part}
          </mark>
        ) : (
          <Fragment key={i}>{part}</Fragment>
        )
      )}
    </>
  )
}

/// ⌘K: jump to any page by title, or create one. Deliberately not animated:
/// it is opened from the keyboard many times a day.
export function QuickSwitcher({
  open,
  onClose,
  workspaceId,
  doc,
  all,
  live,
  readonly,
  onOpen,
}: {
  open: boolean
  onClose: () => void
  workspaceId: string | null
  doc: Y.Doc | null
  all: Page[]
  live: Page[]
  readonly: boolean
  onOpen: (id: string) => void
}) {
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const [hits, setHits] = useState<SearchHit[]>([])
  const ref = useRef<HTMLDialogElement>(null)

  // Page text matches come from Rust's index; titles are filtered here.
  useEffect(() => {
    const q = query.trim()
    if (!open || !workspaceId || q.length < 2) {
      setHits([])
      return
    }
    let stale = false
    const timer = setTimeout(() => {
      api
        .searchPages(workspaceId, q)
        .then((h) => !stale && setHits(h))
        .catch(() => !stale && setHits([]))
    }, 120)
    return () => {
      stale = true
      clearTimeout(timer)
    }
  }, [query, open, workspaceId])

  useEffect(() => {
    const d = ref.current
    if (!d) return
    if (open && !d.open) {
      setQuery('')
      setActive(0)
      d.showModal()
    }
    if (!open && d.open) d.close()
  }, [open])

  const results = useMemo<Result[]>(() => {
    const q = query.trim().toLowerCase()
    const pages = live
      .filter((p) => !q || (p.title || 'untitled').toLowerCase().includes(q))
      .sort((a, b) => {
        if (!q) return b.created - a.created
        const ai = (a.title || 'untitled').toLowerCase().indexOf(q)
        const bi = (b.title || 'untitled').toLowerCase().indexOf(q)
        return ai - bi || a.title.localeCompare(b.title)
      })
      .slice(0, 40)
      .map<Result>((page) => ({
        kind: 'page',
        page,
        path: pagePath(all, page.id)
          .slice(0, -1)
          .map((p) => p.title || 'Untitled')
          .join(' / '),
      }))
    const shown = new Set(pages.map((r) => (r.kind === 'page' ? r.page.id : '')))
    const byId = new Map(live.map((p) => [p.id, p]))
    for (const hit of hits) {
      const page = byId.get(hit.id)
      if (page && !shown.has(hit.id) && hit.snippet) pages.push({ kind: 'text', page, snippet: hit.snippet })
    }
    if (q && !readonly) pages.push({ kind: 'create', title: query.trim() })
    return pages
  }, [query, live, all, readonly, hits])

  const choose = (r: Result | undefined) => {
    if (!r) return
    if (r.kind === 'page' || r.kind === 'text') onOpen(r.page.id)
    else if (doc) onOpen(createPage(doc, { title: r.title }))
    onClose()
  }

  return (
    <dialog
      ref={ref}
      className="kw-dialog mt-[14vh]"
      onCancel={(e) => {
        e.preventDefault()
        onClose()
      }}
      onMouseDown={(e) => e.target === ref.current && onClose()}
      data-testid="quick-switcher"
    >
      {open && (
        <div className="w-[560px] overflow-hidden rounded-lg bg-raised shadow-pop">
          <input
            autoFocus
            value={query}
            placeholder="Search pages or create one…"
            aria-label="Search pages"
            onChange={(e) => {
              setQuery(e.target.value)
              setActive(0)
            }}
            onKeyDown={(e) => {
              if (e.key === 'ArrowDown') {
                e.preventDefault()
                setActive((a) => Math.min(a + 1, results.length - 1))
              } else if (e.key === 'ArrowUp') {
                e.preventDefault()
                setActive((a) => Math.max(a - 1, 0))
              } else if (e.key === 'Enter') {
                e.preventDefault()
                choose(results[active])
              }
            }}
            className="h-12 w-full border-b border-line bg-transparent px-4 text-[15px] text-ink outline-none placeholder:text-ink-3"
          />
          <div className="max-h-[360px] overflow-y-auto p-1.5">
            {results.length === 0 && (
              <p className="px-3 py-6 text-center text-[13px] text-ink-3">No pages match.</p>
            )}
            {results.map((r, i) => (
              <Fragment key={r.kind === 'create' ? 'create' : `${r.kind}-${r.page.id}`}>
              {r.kind === 'text' && results[i - 1]?.kind !== 'text' && (
                <div className="px-2.5 pt-2 pb-1 text-[11px] font-medium text-ink-3">In page text</div>
              )}
              <button
                type="button"
                onMouseMove={() => setActive(i)}
                onClick={() => choose(r)}
                className={`flex w-full items-center gap-2.5 rounded-md px-2.5 text-left text-[13px] ${
                  r.kind === 'text' ? 'py-1.5' : 'h-9'
                } ${i === active ? 'bg-hover' : ''}`}
              >
                {r.kind === 'text' ? (
                  <>
                    <span className="flex w-5 shrink-0 justify-center self-start pt-0.5 text-ink-3">
                      <TextSearch size={14} />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className={`block truncate ${r.page.title ? 'text-ink' : 'text-ink-3'}`}>
                        {r.page.icon ? `${r.page.icon} ` : ''}
                        {r.page.title || 'Untitled'}
                      </span>
                      <span className="line-clamp-2 text-[12px] leading-snug text-ink-2">
                        <Highlight text={r.snippet} query={query} />
                      </span>
                    </span>
                  </>
                ) : r.kind === 'page' ? (
                  <>
                    <span className="flex w-5 justify-center text-[14px]">
                      {r.page.icon || <FileText size={14} className="text-ink-3" />}
                    </span>
                    <span className={`truncate ${r.page.title ? 'text-ink' : 'text-ink-3'}`}>
                      {r.page.title || 'Untitled'}
                    </span>
                    {r.path && <span className="truncate text-[12px] text-ink-3">{r.path}</span>}
                  </>
                ) : (
                  <>
                    <span className="flex w-5 justify-center text-ink-3">
                      <SquarePen size={14} />
                    </span>
                    <span className="truncate text-ink">
                      New page “<span className="font-medium">{r.title}</span>”
                    </span>
                  </>
                )}
                {i === active && <CornerDownLeft size={13} className="ml-auto shrink-0 text-ink-3" />}
              </button>
              </Fragment>
            ))}
          </div>
        </div>
      )}
    </dialog>
  )
}
