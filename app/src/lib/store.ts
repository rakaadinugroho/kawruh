import { create } from 'zustand'
import { type Appearance, applyAppearance, cachedAppearance } from './appearance'
import { closeWorkspaceDocs } from './docs'
import { type UpdateState, findUpdate, installUpdate } from './updater'
import { type Account, type WorkspaceInfo, api, errorMessage } from './ipc'

export type DialogName =
  | null
  | { kind: 'settings'; tab?: 'accounts' | 'appearance' | 'tokens' | 'about' }
  | { kind: 'newWorkspace' }
  | { kind: 'join' }
  | { kind: 'share' }
  | { kind: 'quick' }
  | { kind: 'trash' }
  | { kind: 'templates' }

type Toast = { id: number; text: string; tone: 'info' | 'error' }

type State = {
  workspaces: WorkspaceInfo[]
  accounts: Account[]
  workspaceId: string | null
  pageId: string | null
  sidebarOpen: boolean
  dialog: DialogName
  toasts: Toast[]
  loaded: boolean
  appearance: Appearance
  update: UpdateState

  refresh: () => Promise<void>
  selectWorkspace: (id: string) => void
  openPage: (id: string | null) => void
  setDialog: (d: DialogName) => void
  toggleSidebar: () => void
  setStatus: (workspaceId: string, status: WorkspaceInfo['status']) => void
  toast: (text: string, tone?: Toast['tone']) => void
  fail: (e: unknown) => void
  forgetWorkspace: (id: string) => Promise<void>
  /// Applies a page font/size change; `save` keeps it for next time.
  setAppearance: (patch: Partial<Appearance>, save?: boolean) => void
  /// Looks for a newer release; `manual` reports "up to date" and errors.
  checkForUpdate: (manual?: boolean) => Promise<void>
  /// Installs the found update and restarts.
  applyUpdate: () => Promise<void>
}

let toastId = 0

const remember = (key: string, value: string) => api.settingSet(key, value).catch(() => {})

export const useApp = create<State>((set, get) => ({
  workspaces: [],
  accounts: [],
  workspaceId: null,
  pageId: null,
  sidebarOpen: true,
  dialog: null,
  toasts: [],
  loaded: false,
  appearance: cachedAppearance(),
  update: { kind: 'idle' },

  refresh: async () => {
    const [workspaces, accounts] = await Promise.all([api.workspaces(), api.accounts()])
    const current = get().workspaceId
    const workspaceId =
      current && workspaces.some((w) => w.id === current) ? current : (workspaces[0]?.id ?? null)
    set({ workspaces, accounts, workspaceId, loaded: true })
  },

  selectWorkspace: (id) => {
    if (get().workspaceId === id) return
    set({ workspaceId: id, pageId: null })
    remember('lastWorkspace', id)
  },

  openPage: (id) => {
    set({ pageId: id })
    const ws = get().workspaceId
    if (ws && id) remember(`lastPage:${ws}`, id)
  },

  setDialog: (dialog) => set({ dialog }),
  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),

  setStatus: (workspaceId, status) =>
    set((s) => ({
      workspaces: s.workspaces.map((w) => (w.id === workspaceId ? { ...w, status } : w)),
    })),

  toast: (text, tone = 'info') => {
    const id = ++toastId
    set((s) => ({ toasts: [...s.toasts, { id, text, tone }] }))
    setTimeout(() => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })), 4200)
  },

  fail: (e) => get().toast(errorMessage(e), 'error'),

  forgetWorkspace: async (id) => {
    closeWorkspaceDocs(id)
    await api.forgetWorkspace(id)
    set({ workspaceId: null, pageId: null })
    await get().refresh()
  },

  checkForUpdate: async (manual = false) => {
    const { kind } = get().update
    if (kind === 'checking' || kind === 'installing') return
    set({ update: { kind: 'checking' } })
    try {
      const version = await findUpdate()
      set({ update: version ? { kind: 'available', version } : { kind: 'current' } })
      if (manual && !version) get().toast("You're on the latest version.")
    } catch (e) {
      set({ update: { kind: 'idle' } })
      if (manual) get().fail(e)
    }
  },

  applyUpdate: async () => {
    const current = get().update
    if (current.kind !== 'available') return
    const { version } = current
    set({ update: { kind: 'installing', version, percent: null } })
    try {
      await installUpdate((percent) => set({ update: { kind: 'installing', version, percent } }))
    } catch (e) {
      set({ update: { kind: 'available', version } })
      get().fail(e)
    }
  },

  setAppearance: (patch, save = true) => {
    const appearance = { ...get().appearance, ...patch }
    set({ appearance })
    applyAppearance(appearance)
    if (save) {
      remember('docFont', appearance.font)
      remember('docSize', appearance.size)
    }
  },
}))

export const currentWorkspace = (s: State) => s.workspaces.find((w) => w.id === s.workspaceId) ?? null
