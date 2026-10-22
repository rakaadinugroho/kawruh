import { listen } from '@tauri-apps/api/event'
import { Suspense, lazy, useEffect } from 'react'
import { InboxDialog } from './components/InboxDialog'
import { Onboarding } from './components/Onboarding'
import { PageView } from './components/PageView'
import { QuickSwitcher } from './components/QuickSwitcher'
import { SettingsDialog } from './components/SettingsDialog'
import { ShareDialog } from './components/ShareDialog'
import { Sidebar } from './components/Sidebar'
import { Toasts } from './components/Toasts'
import { TemplatesDialog } from './components/TemplatesDialog'
import { TrashDialog } from './components/TrashDialog'
import { JoinDialog, NewWorkspaceDialog } from './components/WorkspaceDialogs'
import { useDoc, useWorkspaceView } from './lib/hooks'
import { type SyncState, api } from './lib/ipc'
import { parseAppearance } from './lib/appearance'
import { INBOX_EVERY_MS, useInbox } from './lib/inbox'
import { CHECK_EVERY_MS } from './lib/updater'
import { currentWorkspace, useApp } from './lib/store'
import { createPage } from './lib/workspace'

const GraphView = lazy(() => import('./components/GraphView'))

export default function App() {
  const {
    loaded,
    workspaces,
    workspaceId,
    pageId,
    graphOpen,
    sidebarOpen,
    dialog,
    setDialog,
    refresh,
    selectWorkspace,
    openPage,
    showGraph,
    showComments,
    toggleSidebar,
    setStatus,
    toast,
    fail,
  } = useApp()
  const inboxOpen = useInbox((s) => s.open)
  const showInbox = useInbox((s) => s.show)
  const ws = useApp(currentWorkspace)
  const wsHandle = useDoc(workspaceId, workspaceId)
  const view = useWorkspaceView(wsHandle)
  const readonly = ws?.role === 'viewer'
  const page = view.all.find((p) => p.id === pageId) ?? null

  useEffect(() => {
    ;(async () => {
      try {
        await refresh()
        const [font, size] = await Promise.all([api.settingGet('docFont'), api.settingGet('docSize')])
        if (font || size) useApp.getState().setAppearance(parseAppearance(font, size), false)
        const last = await api.settingGet('lastWorkspace')
        if (last && useApp.getState().workspaces.some((w) => w.id === last)) selectWorkspace(last)
        // Pick up role changes made while the app was closed.
        const signedIn = useApp.getState().accounts.filter((a) => a.loggedIn)
        if (signedIn.length) {
          await Promise.allSettled(signedIn.map((a) => api.remoteWorkspaces(a.id)))
          await refresh()
        }
      } catch (e) {
        fail(e)
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Markdown files opened with Kawruh (Finder, file association, command line)
  // become pages of the current workspace, or of a new local one.
  useEffect(() => {
    if (!loaded) return
    const take = async () => {
      try {
        if ((await api.pendingFiles()) === 0) return
        let id = useApp.getState().workspaceId
        if (!id) {
          const created = await api.createWorkspace('Notes', null)
          await refresh()
          useApp.getState().selectWorkspace(created.id)
          id = created.id
        }
        const pages = await api.openPendingFiles(id as string)
        if (pages.length) {
          openPage(pages[pages.length - 1])
          toast(pages.length === 1 ? 'Opened as a page' : `Opened ${pages.length} files as pages`)
        }
      } catch (e) {
        fail(e)
      }
    }
    void take()
    const off = listen('files-opened', () => void take())
    return () => {
      off.then((f) => f())
    }
  }, [loaded, refresh, openPage, toast, fail])

  // Updates: shortly after launch, then every few hours (not in dev builds).
  useEffect(() => {
    if (import.meta.env.DEV) return
    const run = () => void useApp.getState().checkForUpdate()
    const first = setTimeout(run, 5000)
    const every = setInterval(run, CHECK_EVERY_MS)
    return () => {
      clearTimeout(first)
      clearInterval(every)
    }
  }, [])

  // The inbox: on launch, then every minute for each signed-in account.
  useEffect(() => {
    if (!loaded) return
    const accounts = useApp.getState().accounts.filter((a) => a.loggedIn)
    if (accounts.length === 0) return
    accounts.forEach((a) => void useInbox.getState().refresh(a.id))
    const every = setInterval(() => {
      for (const a of useApp.getState().accounts.filter((x) => x.loggedIn)) {
        void useInbox.getState().refresh(a.id)
      }
    }, INBOX_EVERY_MS)
    return () => clearInterval(every)
  }, [loaded])

  // Reopen the page last viewed in this workspace.
  useEffect(() => {
    if (!workspaceId) return
    api
      .settingGet(`lastPage:${workspaceId}`)
      .then((id) => {
        const s = useApp.getState()
        if (id && s.workspaceId === workspaceId && !s.pageId) openPage(id)
      })
      .catch(() => {})
  }, [workspaceId, openPage])

  useEffect(() => {
    const offStatus = listen<{ workspaceId: string; status: SyncState }>('sync-status', (e) => {
      setStatus(e.payload.workspaceId, e.payload.status)
      // Roles can change while we were away; the server list refreshes them.
      const ws = useApp.getState().workspaces.find((w) => w.id === e.payload.workspaceId)
      if (e.payload.status === 'online' && ws?.accountId) {
        api.remoteWorkspaces(ws.accountId).then(refresh).catch(() => {})
      }
    })
    const offError = listen<{ code: string; message: string }>('sync-error', (e) => {
      if (e.payload.code === 'readonly') {
        toast('You can only view this workspace.')
        refresh().catch(() => {})
      } else toast(e.payload.message || 'Sync error', 'error')
    })
    let lastPersistToast = 0
    const offPersist = listen<string>('persist-error', (e) => {
      if (Date.now() - lastPersistToast < 30_000) return
      lastPersistToast = Date.now()
      toast(`Couldn't save to disk (${e.payload}). Kawruh keeps retrying; free up space to be safe.`, 'error')
    })
    return () => {
      offStatus.then((f) => f())
      offError.then((f) => f())
      offPersist.then((f) => f())
    }
  }, [setStatus, toast, refresh])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey)) return
      const key = e.key.toLowerCase()
      if (key === 'k') {
        e.preventDefault()
        setDialog(useApp.getState().dialog?.kind === 'quick' ? null : { kind: 'quick' })
      } else if (key === 'n' && !e.shiftKey && wsHandle && !readonly) {
        e.preventDefault()
        openPage(createPage(wsHandle.doc))
      } else if (key === 'g' && e.shiftKey) {
        e.preventDefault()
        showGraph(!useApp.getState().graphOpen)
      } else if (key === 'm' && e.shiftKey) {
        e.preventDefault()
        showComments(!useApp.getState().commentsOpen)
      } else if (key === '\\') {
        e.preventDefault()
        toggleSidebar()
      } else if (key === ',') {
        e.preventDefault()
        setDialog({ kind: 'settings' })
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [wsHandle, readonly, setDialog, openPage, showGraph, showComments, toggleSidebar])

  if (!loaded) return <div data-tauri-drag-region className="h-full bg-paper" />

  const close = () => setDialog(null)

  return (
    <>
      {workspaces.length === 0 ? (
        <Onboarding />
      ) : (
        <div className="flex h-full">
          {sidebarOpen && <Sidebar handle={wsHandle} view={view} />}
          {graphOpen && workspaceId ? (
            <Suspense fallback={<main className="h-full flex-1 bg-paper" />}>
              <GraphView key={workspaceId} workspaceId={workspaceId} view={view} currentId={pageId} />
            </Suspense>
          ) : (
            <PageView wsHandle={wsHandle} view={view} />
          )}
        </div>
      )}
      <QuickSwitcher
        open={dialog?.kind === 'quick'}
        onClose={close}
        workspaceId={workspaceId}
        doc={wsHandle?.doc ?? null}
        all={view.all}
        live={view.live}
        readonly={readonly}
        onOpen={(id) => openPage(id)}
        onGraph={() => showGraph(true)}
      />
      <SettingsDialog
        open={dialog?.kind === 'settings'}
        tab={dialog?.kind === 'settings' ? dialog.tab : undefined}
        onClose={close}
      />
      <ShareDialog open={dialog?.kind === 'share'} onClose={close} page={page} />
      <NewWorkspaceDialog open={dialog?.kind === 'newWorkspace'} onClose={close} />
      <JoinDialog open={dialog?.kind === 'join'} onClose={close} />
      <TrashDialog
        open={dialog?.kind === 'trash'}
        onClose={close}
        doc={wsHandle?.doc ?? null}
        trash={view.trash}
        readonly={readonly}
        onOpen={(id) => openPage(id)}
      />
      <TemplatesDialog
        open={dialog?.kind === 'templates'}
        onClose={close}
        workspaceId={workspaceId}
        wsHandle={wsHandle}
        live={view.live}
      />
      <InboxDialog
        open={inboxOpen}
        onClose={() => showInbox(false)}
        accountId={ws?.accountId ?? null}
        onOpen={(n) => {
          openPage(n.pageId)
          // The page may still be loading; the panel waits for it and scrolls here.
          useApp.getState().focusComment(n.commentId)
        }}
      />
      <Toasts />
    </>
  )
}
