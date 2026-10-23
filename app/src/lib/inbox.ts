/// The inbox: mentions and replies the user hasn't looked at yet.
///
/// Notifications live on the server, per account. The app polls every minute
/// and keeps the last answer per account, so the sidebar can show a badge
/// without waiting for a sync.
import { create } from 'zustand'
import { type Inbox, type Notification, api } from './ipc'

/// How often the inbox is refreshed.
export const INBOX_EVERY_MS = 60_000

type State = {
  /// The last answer per account id.
  byAccount: Record<string, Inbox>
  open: boolean
  refresh: (accountId: string) => Promise<void>
  markRead: (accountId: string, ids: string[]) => Promise<void>
  show: (open: boolean) => void
}

const empty: Inbox = { items: [], unread: 0 }

/// The empty inbox, one object for the whole app. A selector that builds a
/// new one every time would never settle: React compares the snapshot of a
/// store with the last one and re-renders as long as they differ, which ends
/// in "Maximum update depth exceeded" and a blank window.
export const EMPTY_INBOX: Inbox = empty

/// Picks one account's inbox out of the store. Always the same object for
/// the same state, so it can be used as a store selector.
export const inboxSelector =
  (accountId: string | null | undefined) =>
  (s: State): Inbox =>
    (accountId ? s.byAccount[accountId] : undefined) ?? EMPTY_INBOX

export const useInbox = create<State>((set, get) => ({
  byAccount: {},
  open: false,

  refresh: async (accountId) => {
    try {
      const inbox = await api.notificationsList(accountId)
      set((s) => ({ byAccount: { ...s.byAccount, [accountId]: inbox } }))
    } catch {
      // Offline or signed out: keep what we have.
    }
  },

  markRead: async (accountId, ids) => {
    const before = get().byAccount[accountId] ?? empty
    // Mark them read right away; the server confirms a moment later.
    set((s) => ({
      byAccount: {
        ...s.byAccount,
        [accountId]: {
          items: before.items.map((n) =>
            ids.length === 0 || ids.includes(n.id) ? { ...n, readAt: n.readAt ?? Date.now() } : n
          ),
          unread: ids.length === 0 ? 0 : Math.max(0, before.unread - ids.length),
        },
      },
    }))
    try {
      const res = await api.notificationsRead(accountId, ids)
      set((s) => ({
        byAccount: { ...s.byAccount, [accountId]: { ...(s.byAccount[accountId] ?? empty), unread: res.unread } },
      }))
    } catch {
      // The next poll brings the truth back.
    }
  },

  show: (open) => set({ open }),
}))

/// The inbox of an account, or an empty one.
export const inboxOf = (accountId: string | null | undefined): Inbox =>
  (accountId && useInbox.getState().byAccount[accountId]) || empty

export type { Notification }
