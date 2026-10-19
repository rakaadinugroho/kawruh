import { listen } from '@tauri-apps/api/event'
import { useEffect } from 'react'
import { Onboarding } from './components/Onboarding'
import { PageView } from './components/PageView'
import { QuickSwitcher } from './components/QuickSwitcher'
import { SettingsDialog } from './components/SettingsDialog'
import { ShareDialog } from './components/ShareDialog'
import { Sidebar } from './components/Sidebar'
import { Toasts } from './components/Toasts'
import { TrashDialog } from './components/TrashDialog'
import { JoinDialog, NewWorkspaceDialog } from './components/WorkspaceDialogs'
import { useDoc, useWorkspaceView } from './lib/hooks'
import { type SyncState, api } from './lib/ipc'
import { parseAppearance } from './lib/appearance'
import { currentWorkspace, useApp } from './lib/store'
import { createPage } from './lib/workspace'

export default function App() {
  const {
    loaded,
    workspaces,
    workspaceId,
    pageId,
    sidebarOpen,
    dialog,
    setDialog,
    refresh,
    selectWorkspace,
    openPage,
    toggleSidebar,
    setStatus,
    toast,
    fail,
  } = useApp()
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
  }, [wsHandle, readonly, setDialog, openPage, toggleSidebar])

  if (!loaded) return <div data-tauri-drag-region className="h-full bg-paper" />

  const close = () => setDialog(null)

  return (
    <>
      {workspaces.length === 0 ? (
        <Onboarding />
      ) : (
        <div className="flex h-full">
          {sidebarOpen && <Sidebar handle={wsHandle} view={view} />}
          <PageView wsHandle={wsHandle} view={view} />
        </div>
      )}
      <QuickSwitcher
        open={dialog?.kind === 'quick'}
        onClose={close}
        doc={wsHandle?.doc ?? null}
        all={view.all}
        live={view.live}
        readonly={readonly}
        onOpen={(id) => openPage(id)}
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
      <Toasts />
    </>
  )
}
