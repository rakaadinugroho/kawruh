import {
  ChevronRight,
  ClipboardCopy,
  Download,
  FilePlus2,
  FileText,
  MoreHorizontal,
  PanelLeftOpen,
  RotateCcw,
  Share2,
  SmilePlus,
  SquarePen,
  Trash2,
  Upload,
} from 'lucide-react'
import { Suspense, lazy, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Editor } from '../editor/Editor'
import type { DocHandle } from '../lib/docs'
import { type WorkspaceView, colorFor, useDoc, usePeers } from '../lib/hooks'
import { api } from '../lib/ipc'
import { currentWorkspace, useApp } from '../lib/store'
import { key } from '../lib/platform'
import {
  type Page,
  childrenOf,
  createPage,
  deletePage,
  pagePath,
  renamePage,
  restorePage,
  setPageIcon,
} from '../lib/workspace'
import { Button, IconButton, Menu, copyText, useMenuAnchor } from './ui'

const CanvasEditor = lazy(() => import('../editor/CanvasEditor'))

const ICONS = [
  '📝', '📚', '💡', '✅', '📌', '🗂️', '🧭', '🧪', '🛠️', '🎯', '📅', '🧠',
  '🌱', '🔖', '📎', '🗺️', '💬', '🏁', '⚙️', '🔒', '🧾', '📊', '🖋️', '☕',
]

function Breadcrumbs({ trail, name, onOpen }: { trail: Page[]; name: string; onOpen: (id: string) => void }) {
  const shown = trail.length > 3 ? [trail[0], null, ...trail.slice(-2)] : trail
  return (
    <nav aria-label="Breadcrumb" className="flex min-w-0 items-center gap-0.5 text-[13px]">
      <span className="shrink-0 truncate px-1 text-ink-3">{name}</span>
      {shown.map((p, i) => (
        <span key={p ? p.id : `gap-${i}`} className="flex min-w-0 items-center gap-0.5">
          <ChevronRight size={12} className="shrink-0 text-ink-3" />
          {p ? (
            <button
              type="button"
              onClick={() => onOpen(p.id)}
              className={`press min-w-0 truncate rounded-md px-1.5 py-0.5 hover:bg-hover ${
                i === shown.length - 1 ? 'text-ink' : 'text-ink-2'
              }`}
            >
              {p.icon ? `${p.icon} ` : ''}
              {p.title || 'Untitled'}
            </button>
          ) : (
            <span className="px-1 text-ink-3">…</span>
          )}
        </span>
      ))}
    </nav>
  )
}

function Presence({ handle }: { handle: DocHandle | null }) {
  const peers = usePeers(handle)
  if (peers.length === 0) return null
  const shown = peers.slice(0, 4)
  return (
    <div className="flex items-center pr-1" data-testid="presence" aria-label={`${peers.length} others here`}>
      {shown.map((p) => (
        <span
          key={p.clientId}
          title={p.name}
          className="-ml-1.5 flex size-6 items-center justify-center rounded-full text-[10px] font-semibold text-white ring-2 ring-paper first:ml-0"
          style={{ background: p.color }}
        >
          {p.name
            .split(/\s+/)
            .map((w) => w.replace(/[^\p{L}\p{N}]/gu, '')[0])
            .filter(Boolean)
            .join('')
            .slice(0, 2)
            .toUpperCase()}
        </span>
      ))}
      {peers.length > shown.length && (
        <span className="-ml-1.5 flex size-6 items-center justify-center rounded-full bg-hover-2 text-[10px] font-semibold text-ink-2 ring-2 ring-paper">
          +{peers.length - shown.length}
        </span>
      )}
    </div>
  )
}

function TitleInput({ value, readonly, onChange, onEnter }: {
  value: string
  readonly: boolean
  onChange: (v: string) => void
  onEnter: () => void
}) {
  const ref = useRef<HTMLTextAreaElement>(null)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    el.style.height = '0px'
    el.style.height = `${el.scrollHeight}px`
  }, [value])
  return (
    <textarea
      ref={ref}
      data-testid="page-title"
      value={value}
      readOnly={readonly}
      rows={1}
      placeholder="Untitled"
      aria-label="Page title"
      onChange={(e) => onChange(e.target.value.replace(/\n/g, ' '))}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault()
          onEnter()
        }
      }}
      className="w-full resize-none overflow-hidden bg-transparent font-serif text-[40px] leading-[1.15] font-[650] tracking-[-0.022em] text-ink outline-none placeholder:text-ink-3/60 [font-variation-settings:'opsz'_60]"
    />
  )
}

export function PageView({ wsHandle, view }: { wsHandle: DocHandle | null; view: WorkspaceView }) {
  const { pageId, openPage, sidebarOpen, toggleSidebar, setDialog, accounts, fail, toast } = useApp()
  const ws = useApp(currentWorkspace)
  const page = view.all.find((p) => p.id === pageId) ?? null
  const handle = useDoc(page ? page.id : null, ws?.id ?? null)
  const [canvasId, setCanvasId] = useState<string | null>(null)
  const more = useMenuAnchor()
  const iconMenu = useMenuAnchor()
  const readonly = ws?.role === 'viewer'
  const account = accounts.find((a) => a.id === ws?.accountId)
  const trail = useMemo(() => (page ? pagePath(view.all, page.id) : []), [view.all, page])
  const children = useMemo(() => (page ? childrenOf(view.live, page.id) : []), [view.live, page])
  const inTrash = page ? !view.live.some((p) => p.id === page.id) : false

  // Name and color shown on our cursor to collaborators. Docs start with a null
  // awareness state (nothing to announce) and setLocalStateField is a no-op on
  // null, so set the whole state; y-prosemirror then adds its cursor field.
  useEffect(() => {
    if (!handle || !account) return
    handle.awareness.setLocalState({
      ...(handle.awareness.getLocalState() ?? {}),
      user: { name: account.displayName || account.username, color: colorFor(account.userId) },
    })
  }, [handle, account])

  useEffect(() => setCanvasId(null), [pageId])

  const focusEditor = () => {
    const pm = document.querySelector<HTMLElement>('.kw-editor .ProseMirror')
    pm?.focus()
  }

  const exportMd = async () => {
    if (!ws || !page) return
    try {
      if (await api.exportFile(ws.id, page.id)) toast('Exported markdown')
    } catch (e) {
      fail(e)
    }
  }

  const importMd = async () => {
    if (!ws || !page) return
    try {
      const id = await api.importFile(ws.id, page.id)
      if (id) openPage(id)
    } catch (e) {
      fail(e)
    }
  }

  const copyMd = async () => {
    if (!ws || !page) return
    try {
      await copyText(await api.exportMarkdown(ws.id, page.id))
      toast('Copied as markdown')
    } catch (e) {
      fail(e)
    }
  }

  return (
    <main className="flex h-full min-w-0 flex-1 flex-col bg-paper" data-testid="page-view">
      <header
        data-tauri-drag-region
        className={`flex h-11 shrink-0 items-center gap-2 pr-3 ${sidebarOpen ? 'pl-3' : 'pl-[78px]'}`}
      >
        {!sidebarOpen && (
          <IconButton label={`Show sidebar (${key('\\')})`} onClick={toggleSidebar}>
            <PanelLeftOpen size={15} />
          </IconButton>
        )}
        <div data-tauri-drag-region className="flex min-w-0 flex-1 items-center">
          {page && <Breadcrumbs trail={trail} name={view.name} onOpen={(id) => openPage(id)} />}
        </div>
        <Presence handle={handle} />
        {ws && (
          <Button variant="ghost" className="h-7 px-2.5" onClick={() => setDialog({ kind: 'share' })} data-testid="share-button">
            <Share2 size={14} />
            Share
          </Button>
        )}
        {page && (
          <IconButton label="Page actions" onClick={(e) => more.openFrom(e.currentTarget)} data-testid="page-menu">
            <MoreHorizontal size={16} />
          </IconButton>
        )}
        <Menu
          at={more.at}
          onClose={more.close}
          items={[
            {
              label: 'Add sub-page',
              icon: <FilePlus2 size={14} />,
              disabled: readonly || !wsHandle,
              onSelect: () => wsHandle && page && openPage(createPage(wsHandle.doc, { parent: page.id })),
            },
            { label: 'Copy as markdown', icon: <ClipboardCopy size={14} />, onSelect: copyMd },
            { label: 'Export markdown…', icon: <Download size={14} />, onSelect: exportMd },
            {
              label: 'Import markdown as sub-page…',
              icon: <Upload size={14} />,
              disabled: readonly,
              onSelect: importMd,
            },
            'separator',
            {
              label: 'Move to trash',
              icon: <Trash2 size={14} />,
              danger: true,
              disabled: readonly || !wsHandle,
              onSelect: () => {
                if (wsHandle && page) deletePage(wsHandle.doc, page.id)
                openPage(null)
              },
            },
          ]}
        />
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto" data-testid="page-scroll">
        {!page ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 pb-24 text-center">
            <p className="font-serif text-[22px] text-ink-2">Nothing open.</p>
            <p className="max-w-[300px] text-[13px] text-ink-3">
              Pick a page in the sidebar, press {key('K')} to search, or start a new one.
            </p>
            {!readonly && wsHandle && (
              <Button variant="primary" onClick={() => openPage(createPage(wsHandle.doc))}>
                <SquarePen size={14} />
                New page
              </Button>
            )}
          </div>
        ) : (
          <article className="mx-auto w-full max-w-[784px] px-16 pt-14 pb-[35vh]">
            {inTrash && (
              <div className="mb-6 flex items-center justify-between gap-3 rounded-lg bg-hover px-4 py-2.5 text-[13px] text-ink-2">
                This page is in the trash.
                {!readonly && wsHandle && (
                  <Button variant="subtle" className="h-7" onClick={() => restorePage(wsHandle.doc, page.id)}>
                    <RotateCcw size={13} />
                    Restore
                  </Button>
                )}
              </div>
            )}
            <div className="group mb-1 flex h-8 items-end">
              {page.icon ? (
                <button
                  type="button"
                  aria-label="Change icon"
                  disabled={readonly}
                  onClick={(e) => iconMenu.openFrom(e.currentTarget)}
                  className="press -ml-1 rounded-md px-1 text-[30px] leading-none hover:bg-hover"
                >
                  {page.icon}
                </button>
              ) : (
                !readonly && (
                  <Button
                    variant="ghost"
                    className="invisible h-7 px-2 text-[12px] group-hover:visible"
                    onClick={(e) => iconMenu.openFrom(e.currentTarget)}
                  >
                    <SmilePlus size={14} />
                    Add icon
                  </Button>
                )
              )}
              {iconMenu.at && wsHandle && (
                <IconPicker
                  at={iconMenu.at}
                  onClose={iconMenu.close}
                  onPick={(icon) => setPageIcon(wsHandle.doc, page.id, icon)}
                  hasIcon={!!page.icon}
                />
              )}
            </div>
            <TitleInput
              value={page.title}
              readonly={readonly || !wsHandle}
              onChange={(v) => wsHandle && renamePage(wsHandle.doc, page.id, v)}
              onEnter={focusEditor}
            />
            <div className="mt-4">
              {handle && handle.isReady() ? (
                <Editor handle={handle} readonly={readonly} onOpenCanvas={setCanvasId} />
              ) : (
                <div className="h-40" />
              )}
            </div>
            {children.length > 0 && (
              <section className="mt-6 border-t border-line pt-4" aria-label="Sub-pages">
                <h3 className="mb-2 text-[11px] font-semibold tracking-[0.04em] text-ink-3 uppercase">
                  Inside this page
                </h3>
                <div className="flex flex-col">
                  {children.map((c) => (
                    <button
                      key={c.id}
                      type="button"
                      onClick={() => openPage(c.id)}
                      className="press -mx-2 flex h-8 items-center gap-2 rounded-md px-2 text-left font-serif text-[15px] text-ink hover:bg-hover"
                    >
                      <span className="flex w-5 justify-center text-[14px]">
                        {c.icon || <FileText size={14} className="text-ink-3" />}
                      </span>
                      <span className={c.title ? '' : 'text-ink-3'}>{c.title || 'Untitled'}</span>
                    </button>
                  ))}
                </div>
              </section>
            )}
          </article>
        )}
      </div>

      {canvasId && handle && (
        <Suspense fallback={<div className="fixed inset-0 z-40 bg-paper" />}>
          <CanvasEditor
            doc={handle.doc}
            canvasId={canvasId}
            readonly={readonly}
            onClose={() => setCanvasId(null)}
          />
        </Suspense>
      )}
    </main>
  )
}

function IconPicker({ at, onClose, onPick, hasIcon }: {
  at: { x: number; y: number }
  onClose: () => void
  onPick: (icon: string) => void
  hasIcon: boolean
}) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose()
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey)
    }
  }, [onClose])
  return (
    <div
      ref={ref}
      className="kw-menu fixed z-50 w-[248px] rounded-lg bg-raised p-2 shadow-pop"
      style={{ left: at.x, top: at.y }}
    >
      <div className="grid grid-cols-8 gap-0.5">
        {ICONS.map((icon) => (
          <button
            key={icon}
            type="button"
            onClick={() => {
              onPick(icon)
              onClose()
            }}
            className="press flex size-7 items-center justify-center rounded-md text-[17px] hover:bg-hover"
          >
            {icon}
          </button>
        ))}
      </div>
      {hasIcon && (
        <button
          type="button"
          onClick={() => {
            onPick('')
            onClose()
          }}
          className="press mt-1.5 h-7 w-full rounded-md text-[12px] text-ink-2 hover:bg-hover"
        >
          Remove icon
        </button>
      )}
    </div>
  )
}
