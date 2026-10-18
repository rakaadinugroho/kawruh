import { CornerDownLeft, FileText, SquarePen } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import type * as Y from 'yjs'
import { type Page, createPage, pagePath } from '../lib/workspace'

type Result = { kind: 'page'; page: Page; path: string } | { kind: 'create'; title: string }

/// ⌘K: jump to any page by title, or create one. Deliberately not animated:
/// it is opened from the keyboard many times a day.
export function QuickSwitcher({
  open,
  onClose,
  doc,
  all,
  live,
  readonly,
  onOpen,
}: {
  open: boolean
  onClose: () => void
  doc: Y.Doc | null
  all: Page[]
  live: Page[]
  readonly: boolean
  onOpen: (id: string) => void
}) {
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)
  const ref = useRef<HTMLDialogElement>(null)

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
    if (q && !readonly) pages.push({ kind: 'create', title: query.trim() })
    return pages
  }, [query, live, all, readonly])

  const choose = (r: Result | undefined) => {
    if (!r) return
    if (r.kind === 'page') onOpen(r.page.id)
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
              <button
                key={r.kind === 'page' ? r.page.id : 'create'}
                type="button"
                onMouseMove={() => setActive(i)}
                onClick={() => choose(r)}
                className={`flex h-9 w-full items-center gap-2.5 rounded-md px-2.5 text-left text-[13px] ${
                  i === active ? 'bg-hover' : ''
                }`}
              >
                {r.kind === 'page' ? (
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
            ))}
          </div>
        </div>
      )}
    </dialog>
  )
}
