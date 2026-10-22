import {
  Check,
  ChevronsUpDown,
  CloudDownload,
  Download,
  LayoutTemplate,
  PanelLeftClose,
  Plus,
  Search,
  Settings,
  SquarePen,
  Trash2,
  Upload,
  Waypoints,
} from 'lucide-react'
import type { DocHandle } from '../lib/docs'
import type { WorkspaceView } from '../lib/hooks'
import { type SyncState, type WorkspaceInfo, api } from '../lib/ipc'
import { currentWorkspace, useApp } from '../lib/store'
import { createPage } from '../lib/workspace'
import { graphKey, key, thisDevice } from '../lib/platform'
import { PageTree } from './PageTree'
import { IconButton, Kbd, Menu, type MenuItem, useMenuAnchor } from './ui'

/// Offers a downloaded-and-verified restart into a newer release.
function UpdateBanner() {
  const update = useApp((s) => s.update)
  const applyUpdate = useApp((s) => s.applyUpdate)
  if (update.kind !== 'available' && update.kind !== 'installing') return null
  const installing = update.kind === 'installing'
  return (
    <div className="mb-1 flex items-center gap-2 rounded-md bg-accent-soft px-2.5 py-2 text-[12px]">
      <span className="min-w-0 flex-1 text-ink">
        {installing
          ? `Updating${update.percent === null ? '…' : ` ${update.percent}%`}`
          : `Kawruh ${update.version} is ready`}
      </span>
      {!installing && (
        <button
          type="button"
          onClick={() => void applyUpdate()}
          className="press shrink-0 rounded-[5px] bg-accent px-2 py-1 text-[11px] font-medium text-accent-ink"
        >
          Restart to update
        </button>
      )}
    </div>
  )
}

const statusCopy: Record<SyncState, string> = {
  local: `On ${thisDevice} only`,
  online: 'Synced',
  connecting: 'Connecting…',
  offline: `Offline, saved on ${thisDevice}`,
  unauthorized: 'Signed out of server',
}

const statusDot: Record<SyncState, string> = {
  local: 'bg-ink-3',
  online: 'bg-[oklch(0.65_0.14_150)]',
  connecting: 'bg-amber',
  offline: 'bg-amber',
  unauthorized: 'bg-danger',
}

export function host(url: string | null) {
  if (!url) return ''
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

function Monogram({ name }: { name: string }) {
  return (
    <span className="flex size-[22px] shrink-0 items-center justify-center rounded-[5px] bg-accent font-serif text-[12px] font-semibold text-accent-ink">
      {(name.trim()[0] ?? 'K').toUpperCase()}
    </span>
  )
}

export function StatusLine({ ws }: { ws: WorkspaceInfo }) {
  return (
    <span className="flex items-center gap-1.5 text-[12px] text-ink-3" data-testid="sync-status">
      <span className={`size-1.5 rounded-full ${statusDot[ws.status]}`} />
      {statusCopy[ws.status]}
    </span>
  )
}

export function Sidebar({ handle, view }: { handle: DocHandle | null; view: WorkspaceView }) {
  const {
    workspaces,
    pageId,
    graphOpen,
    selectWorkspace,
    openPage,
    showGraph,
    setDialog,
    toggleSidebar,
    toast,
    fail,
  } = useApp()
  const ws = useApp(currentWorkspace)
  const switcher = useMenuAnchor()
  const readonly = ws?.role === 'viewer'

  const importNotes = async () => {
    if (!ws) return
    try {
      const res = await api.importWorkspace(ws.id, '')
      if (!res) return
      if (res.root) openPage(res.root)
      toast(`Imported ${res.pages} ${res.pages === 1 ? 'page' : 'pages'}`)
    } catch (e) {
      fail(e)
    }
  }

  const exportAll = async () => {
    if (!ws) return
    try {
      const path = await api.exportWorkspace(ws.id)
      if (path) toast(`Exported to ${path}`)
    } catch (e) {
      fail(e)
    }
  }

  const switchItems: MenuItem[] = [
    ...workspaces.map((w) => ({
      label: w.name || 'Untitled workspace',
      hint: w.serverUrl ? host(w.serverUrl) : 'Local',
      icon: w.id === ws?.id ? <Check size={14} /> : undefined,
      onSelect: () => selectWorkspace(w.id),
    })),
    'separator',
    {
      label: 'Import Notion or Obsidian…',
      icon: <Upload size={14} />,
      disabled: !ws || readonly,
      onSelect: importNotes,
    },
    {
      label: 'Export workspace…',
      icon: <Download size={14} />,
      disabled: !ws,
      onSelect: exportAll,
    },
    'separator',
    {
      label: 'New workspace…',
      icon: <Plus size={14} />,
      onSelect: () => setDialog({ kind: 'newWorkspace' }),
    },
    {
      label: 'Add from server…',
      icon: <CloudDownload size={14} />,
      onSelect: () => setDialog({ kind: 'join' }),
    },
    {
      label: 'Settings',
      icon: <Settings size={14} />,
      hint: key(','),
      onSelect: () => setDialog({ kind: 'settings' }),
    },
  ]

  const newPage = () => {
    if (!handle) return
    openPage(createPage(handle.doc))
  }

  return (
    <aside className="flex h-full w-[260px] shrink-0 flex-col bg-sidebar" data-testid="sidebar">
      <div data-tauri-drag-region className="flex h-[38px] shrink-0 items-center justify-end px-2">
        <IconButton label={`Hide sidebar (${key('\\')})`} onClick={toggleSidebar}>
          <PanelLeftClose size={15} />
        </IconButton>
      </div>

      <div className="px-2">
        <button
          type="button"
          data-testid="workspace-switcher"
          onClick={(e) => switcher.openFrom(e.currentTarget)}
          className="press flex h-9 w-full items-center gap-2 rounded-md px-1.5 text-left hover:bg-hover"
        >
          <Monogram name={view.name || ws?.name || ''} />
          <span className="min-w-0 flex-1 truncate text-[13px] font-semibold text-ink">
            {view.name || ws?.name || 'Kawruh'}
          </span>
          <ChevronsUpDown size={14} className="text-ink-3" />
        </button>
        <Menu at={switcher.at} items={switchItems} onClose={switcher.close} width={260} />

        <button
          type="button"
          onClick={() => setDialog({ kind: 'quick' })}
          className="press mt-1 flex h-7 w-full items-center gap-2 rounded-md px-2 text-[13px] text-ink-2 hover:bg-hover"
        >
          <Search size={14} className="text-ink-3" />
          <span className="flex-1 text-left">Search</span>
          <Kbd>{key('K')}</Kbd>
        </button>
        <button
          type="button"
          data-testid="graph-button"
          aria-pressed={graphOpen}
          onClick={() => showGraph(!graphOpen)}
          disabled={!ws}
          className={`press flex h-7 w-full items-center gap-2 rounded-md px-2 text-[13px] hover:bg-hover ${
            graphOpen ? 'bg-hover text-ink' : 'text-ink-2'
          }`}
        >
          <Waypoints size={14} className={graphOpen ? 'text-ink' : 'text-ink-3'} />
          <span className="flex-1 text-left">Graph</span>
          <Kbd>{graphKey}</Kbd>
        </button>
        {!readonly && (
          <button
            type="button"
            data-testid="new-page"
            onClick={newPage}
            disabled={!handle}
            className="press flex h-7 w-full items-center gap-2 rounded-md px-2 text-[13px] text-ink-2 hover:bg-hover"
          >
            <SquarePen size={14} className="text-ink-3" />
            <span className="flex-1 text-left">New page</span>
            <Kbd>{key('N')}</Kbd>
          </button>
        )}
        {!readonly && (
          <button
            type="button"
            onClick={() => setDialog({ kind: 'templates' })}
            disabled={!handle}
            className="press flex h-7 w-full items-center gap-2 rounded-md px-2 text-[13px] text-ink-2 hover:bg-hover"
          >
            <LayoutTemplate size={14} className="text-ink-3" />
            <span className="flex-1 text-left">From template…</span>
          </button>
        )}
      </div>

      <div className="mt-4 flex items-center justify-between px-4 pb-1">
        <span className="text-[11px] font-semibold tracking-[0.04em] text-ink-3 uppercase">Pages</span>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2">
        {handle && ws && (
          <PageTree
            workspaceId={ws.id}
            doc={handle.doc}
            all={view.all}
            live={view.live}
            currentPageId={pageId}
            readonly={readonly}
            onOpen={(id) => openPage(id || null)}
          />
        )}
      </div>

      <div className="flex shrink-0 flex-col gap-1 border-t border-line px-2 py-2">
        <UpdateBanner />
        <button
          type="button"
          onClick={() => setDialog({ kind: 'trash' })}
          className="press flex h-7 w-full items-center gap-2 rounded-md px-2 text-[13px] text-ink-2 hover:bg-hover"
        >
          <Trash2 size={14} className="text-ink-3" />
          <span className="flex-1 text-left">Trash</span>
          {view.trash.length > 0 && (
            <span className="text-[11px] tabular-nums text-ink-3">{view.trash.length}</span>
          )}
        </button>
        {ws && (
          <div className="flex h-6 items-center px-2">
            <StatusLine ws={ws} />
          </div>
        )}
      </div>
    </aside>
  )
}
