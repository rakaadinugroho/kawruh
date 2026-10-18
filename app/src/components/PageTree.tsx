import { ChevronRight, FilePlus2, FileText, MoreHorizontal, Pencil, Plus, Trash2 } from 'lucide-react'
import { type DragEvent, useEffect, useRef, useState } from 'react'
import type * as Y from 'yjs'
import { useApp } from '../lib/store'
import {
  type Page,
  childrenOf,
  createPage,
  deletePage,
  movePage,
  pagePath,
  renamePage,
} from '../lib/workspace'
import { Menu, useMenuAnchor } from './ui'

type Zone = 'before' | 'after' | 'inside'

const DRAG_TYPE = 'application/x-kawruh-page'

function loadExpanded(workspaceId: string): Set<string> {
  try {
    const raw = localStorage.getItem(`kawruh.expanded.${workspaceId}`)
    return new Set(raw ? (JSON.parse(raw) as string[]) : [])
  } catch {
    return new Set()
  }
}

function saveExpanded(workspaceId: string, set: Set<string>) {
  try {
    localStorage.setItem(`kawruh.expanded.${workspaceId}`, JSON.stringify([...set]))
  } catch {
    // Storage may be unavailable; expansion just won't persist.
  }
}

type Props = {
  workspaceId: string
  doc: Y.Doc
  all: Page[]
  live: Page[]
  currentPageId: string | null
  readonly: boolean
  onOpen: (id: string) => void
}

export function PageTree({ workspaceId, doc, all, live, currentPageId, readonly, onOpen }: Props) {
  const [expanded, setExpanded] = useState(() => loadExpanded(workspaceId))
  const [drop, setDrop] = useState<{ id: string; zone: Zone } | null>(null)
  const [renaming, setRenaming] = useState<string | null>(null)
  const [menuPage, setMenuPage] = useState<Page | null>(null)
  const menu = useMenuAnchor()
  const dragId = useRef<string | null>(null)
  const toast = useApp((s) => s.toast)

  useEffect(() => setExpanded(loadExpanded(workspaceId)), [workspaceId])

  // Reveal the open page once per navigation (not on every tree change, or a
  // folder you collapsed would spring open again whenever anyone types).
  const revealed = useRef<string | null>(null)
  useEffect(() => {
    if (!currentPageId || revealed.current === currentPageId) return
    const path = pagePath(all, currentPageId)
    if (path.length === 0) return
    revealed.current = currentPageId
    const ancestors = path.slice(0, -1).map((p) => p.id)
    if (ancestors.some((id) => !expanded.has(id))) {
      const next = new Set(expanded)
      ancestors.forEach((id) => next.add(id))
      setExpanded(next)
      saveExpanded(workspaceId, next)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentPageId, all])

  const toggle = (id: string, open?: boolean) => {
    const next = new Set(expanded)
    if (open ?? !next.has(id)) next.add(id)
    else next.delete(id)
    setExpanded(next)
    saveExpanded(workspaceId, next)
  }

  const addChild = (parent: string) => {
    const id = createPage(doc, { parent })
    if (parent) toggle(parent, true)
    onOpen(id)
  }

  const zoneOf = (e: DragEvent<HTMLElement>): Zone => {
    const r = e.currentTarget.getBoundingClientRect()
    const y = (e.clientY - r.top) / r.height
    return y < 0.28 ? 'before' : y > 0.72 ? 'after' : 'inside'
  }

  const dropOn = (target: Page | null, zone: Zone) => {
    const id = dragId.current
    dragId.current = null
    setDrop(null)
    if (!id || id === target?.id) return
    let ok: boolean
    if (!target) {
      ok = movePage(doc, id, '')
    } else if (zone === 'inside') {
      ok = movePage(doc, id, target.id)
      if (ok) toggle(target.id, true)
    } else {
      const siblings = childrenOf(live, target.parent).filter((p) => p.id !== id)
      let index = siblings.findIndex((p) => p.id === target.id)
      if (zone === 'after') index += 1
      ok = movePage(doc, id, target.parent, index)
    }
    if (!ok) toast("A page can't move inside itself.")
  }

  const rows = (parent: string, depth: number): React.ReactNode =>
    childrenOf(live, parent).map((page) => {
      const kids = childrenOf(live, page.id)
      const open = expanded.has(page.id)
      const selected = page.id === currentPageId
      const isDrop = drop?.id === page.id
      return (
        <div key={page.id} role="treeitem" aria-expanded={kids.length ? open : undefined}>
          <div
            data-testid="tree-row"
            data-page-id={page.id}
            draggable={!readonly && renaming !== page.id}
            onDragStart={(e) => {
              dragId.current = page.id
              e.dataTransfer.setData(DRAG_TYPE, page.id)
              e.dataTransfer.effectAllowed = 'move'
            }}
            onDragEnd={() => {
              dragId.current = null
              setDrop(null)
            }}
            onDragOver={(e) => {
              if (!dragId.current || dragId.current === page.id) return
              e.preventDefault()
              const zone = zoneOf(e)
              if (drop?.id !== page.id || drop.zone !== zone) setDrop({ id: page.id, zone })
            }}
            onDragLeave={() => isDrop && setDrop(null)}
            onDrop={(e) => {
              e.preventDefault()
              dropOn(page, zoneOf(e))
            }}
            onClick={() => onOpen(page.id)}
            onContextMenu={(e) => {
              e.preventDefault()
              if (readonly) return
              setMenuPage(page)
              menu.openAt(e.clientX, e.clientY)
            }}
            className={`group relative flex h-7 items-center gap-1 rounded-md pr-1 text-[13px] ${
              selected ? 'bg-accent-soft font-medium text-ink' : 'text-ink-2 hover:bg-hover'
            } ${isDrop && drop.zone === 'inside' ? 'shadow-[inset_0_0_0_1.5px_var(--accent)]' : ''}`}
            style={{ paddingLeft: 6 + depth * 14 }}
          >
            {isDrop && drop.zone !== 'inside' && (
              <span
                className={`pointer-events-none absolute right-1 left-2 h-0.5 rounded-full bg-accent ${
                  drop.zone === 'before' ? '-top-px' : '-bottom-px'
                }`}
              />
            )}
            <button
              type="button"
              aria-label={open ? 'Collapse' : 'Expand'}
              onClick={(e) => {
                e.stopPropagation()
                toggle(page.id)
              }}
              className={`press flex size-5 shrink-0 items-center justify-center rounded-sm text-ink-3 hover:bg-hover-2 ${
                kids.length ? '' : 'invisible'
              }`}
            >
              <ChevronRight
                size={13}
                className={`transition-transform duration-150 ${open ? 'rotate-90' : ''}`}
              />
            </button>
            <span className="flex w-5 shrink-0 justify-center text-[14px] leading-none">
              {page.icon || <FileText size={14} className="text-ink-3" />}
            </span>
            {renaming === page.id ? (
              <input
                autoFocus
                defaultValue={page.title}
                aria-label="Page title"
                onClick={(e) => e.stopPropagation()}
                onBlur={(e) => {
                  renamePage(doc, page.id, e.currentTarget.value.trim())
                  setRenaming(null)
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') e.currentTarget.blur()
                  if (e.key === 'Escape') setRenaming(null)
                }}
                className="h-6 min-w-0 flex-1 rounded-sm bg-paper px-1 text-[13px] text-ink shadow-[inset_0_0_0_1.5px_var(--accent)] outline-none"
              />
            ) : (
              <span className={`min-w-0 flex-1 truncate ${page.title ? '' : 'text-ink-3'}`}>
                {page.title || 'Untitled'}
              </span>
            )}
            {!readonly && renaming !== page.id && (
              <span className="hidden shrink-0 items-center group-focus-within:flex group-hover:flex">
                <button
                  type="button"
                  aria-label="Page actions"
                  onClick={(e) => {
                    e.stopPropagation()
                    setMenuPage(page)
                    menu.openFrom(e.currentTarget)
                  }}
                  className="press flex size-5 items-center justify-center rounded-sm text-ink-3 hover:bg-hover-2 hover:text-ink"
                >
                  <MoreHorizontal size={14} />
                </button>
                <button
                  type="button"
                  aria-label="Add sub-page"
                  data-testid="add-subpage"
                  onClick={(e) => {
                    e.stopPropagation()
                    addChild(page.id)
                  }}
                  className="press flex size-5 items-center justify-center rounded-sm text-ink-3 hover:bg-hover-2 hover:text-ink"
                >
                  <Plus size={14} />
                </button>
              </span>
            )}
          </div>
          {open && kids.length > 0 && <div role="group">{rows(page.id, depth + 1)}</div>}
        </div>
      )
    })

  return (
    <div role="tree" aria-label="Pages" className="flex min-h-full flex-col">
      {rows('', 0)}
      {live.length === 0 && (
        <p className="px-2 py-1 text-[12px] text-ink-3">No pages yet.</p>
      )}
      <div
        className="min-h-10 flex-1"
        onDragOver={(e) => {
          if (dragId.current) e.preventDefault()
        }}
        onDrop={(e) => {
          e.preventDefault()
          dropOn(null, 'after')
        }}
      />
      <Menu
        at={menu.at}
        onClose={menu.close}
        items={
          menuPage
            ? [
                {
                  label: 'Add sub-page',
                  icon: <FilePlus2 size={14} />,
                  onSelect: () => addChild(menuPage.id),
                },
                {
                  label: 'Rename',
                  icon: <Pencil size={14} />,
                  onSelect: () => setRenaming(menuPage.id),
                },
                'separator',
                {
                  label: 'Move to trash',
                  icon: <Trash2 size={14} />,
                  danger: true,
                  onSelect: () => {
                    deletePage(doc, menuPage.id)
                    if (
                      currentPageId &&
                      pagePath(all, currentPageId).some((p) => p.id === menuPage.id)
                    )
                      onOpen('')
                  },
                },
              ]
            : []
        }
      />
    </div>
  )
}
