/// Typed wrappers for the Tauri commands in app/src-tauri/src/commands.rs.
import { Channel, invoke } from '@tauri-apps/api/core'

export type Role = 'owner' | 'editor' | 'viewer'
export type SyncState = 'local' | 'online' | 'offline' | 'connecting' | 'unauthorized'

export type WorkspaceInfo = {
  id: string
  name: string
  accountId: string | null
  role: Role
  serverUrl: string | null
  username: string | null
  status: SyncState
}

export type Account = {
  id: string
  serverUrl: string
  userId: string
  username: string
  displayName: string
  loggedIn: boolean
}

/// What a link card shows; fields are absent when the page doesn't say.
export type LinkMeta = {
  title?: string
  description?: string
  image?: string
  icon?: string
  site?: string
}

export type SearchHit = { id: string; title: string; snippet: string; inTitle: boolean }
export type PageRef = { id: string; title: string }
export type HistoryEntry = { id: number; ts: number; preview: string }

/// One page laid out for PDF export (see page_print_data).
export type PrintPage = {
  id: string
  title: string
  icon: string
  depth: number
  markdown: string
  canvases: Record<string, string>
  links: Record<string, string>
}

/// What workspace_import created; `root` is the first page.
export type Imported = { pages: number; root: string | null }

export type RemoteWorkspace ={ id: string; name: string; role: Role; attached: boolean }
export type Member = { userId: string; username: string; displayName: string; role: Role }
export type Share = { token: string; pageId: string; url: string; createdAt?: number | null }
export type ApiToken = {
  id: string
  name: string
  createdAt?: number | null
  lastUsedAt?: number | null
  token?: string | null
}
export type DocEvent = { type: 'update' | 'awareness'; update: string }
export type AppInfo = { version: string; dataDir: string }

export type CommandError = { message: string; code: string }

export function errorMessage(e: unknown): string {
  if (typeof e === 'string') return e
  if (e && typeof e === 'object' && 'message' in e) return String((e as CommandError).message)
  return 'Something failed. Check the server and try again.'
}

export const api = {
  appInfo: () => invoke<AppInfo>('app_info'),

  workspaces: () => invoke<WorkspaceInfo[]>('workspaces_list'),
  createWorkspace: (name: string, accountId: string | null) =>
    invoke<WorkspaceInfo>('workspace_create', { name, accountId }),
  forgetWorkspace: (workspaceId: string) => invoke<void>('workspace_forget', { workspaceId }),
  publishWorkspace: (workspaceId: string, accountId: string) =>
    invoke<WorkspaceInfo>('workspace_publish', { workspaceId, accountId }),
  attachWorkspace: (accountId: string, workspaceId: string) =>
    invoke<WorkspaceInfo>('workspace_attach', { accountId, workspaceId }),
  remoteWorkspaces: (accountId: string) =>
    invoke<RemoteWorkspace[]>('remote_workspaces', { accountId }),

  accounts: () => invoke<Account[]>('accounts_list'),
  login: (args: {
    serverUrl: string
    username: string
    password: string
    register: boolean
    displayName?: string
    invite?: string
  }) => invoke<Account>('account_login', args),
  /// A single-use sign-up code for this account's server (valid 7 days).
  inviteCreate: (accountId: string) => invoke<{ code: string }>('invite_create', { accountId }),
  logout: (accountId: string) => invoke<void>('account_logout', { accountId }),

  members: (workspaceId: string) => invoke<Member[]>('members_list', { workspaceId }),
  addMember: (workspaceId: string, username: string, role: Role) =>
    invoke<Member>('member_add', { workspaceId, username, role }),
  removeMember: (workspaceId: string, userId: string) =>
    invoke<void>('member_remove', { workspaceId, userId }),

  shares: (workspaceId: string) => invoke<Share[]>('shares_list', { workspaceId }),
  createShare: (workspaceId: string, pageId: string) =>
    invoke<Share>('share_create', { workspaceId, pageId }),
  deleteShare: (workspaceId: string, token: string) =>
    invoke<void>('share_delete', { workspaceId, token }),

  tokens: (accountId: string) => invoke<ApiToken[]>('tokens_list', { accountId }),
  createToken: (accountId: string, name: string) =>
    invoke<ApiToken>('token_create', { accountId, name }),
  deleteToken: (accountId: string, tokenId: string) =>
    invoke<void>('token_delete', { accountId, tokenId }),

  docOpen: (docId: string, workspaceId: string, channel: Channel<DocEvent>) =>
    invoke<{ view: number; state: string }>('doc_open', { docId, workspaceId, channel }),
  docClose: (docId: string, view: number) => invoke<void>('doc_close', { docId, view }),
  docApply: (docId: string, view: number, update: string) =>
    invoke<void>('doc_apply', { docId, view, update }),
  awarenessSend: (workspaceId: string, docId: string, update: string) =>
    invoke<void>('awareness_send', { workspaceId, docId, update }),

  exportMarkdown: (workspaceId: string, pageId: string) =>
    invoke<string>('page_export_markdown', { workspaceId, pageId }),
  importMarkdown: (workspaceId: string, parent: string, title: string, markdownText: string) =>
    invoke<string>('page_import_markdown', { workspaceId, parent, title, markdownText }),
  /// Native save dialog + write, both on the Rust side. Null when cancelled.
  exportFile: (workspaceId: string, pageId: string) =>
    invoke<string | null>('page_export_file', { workspaceId, pageId }),
  /// Native open dialog + import as a sub-page. Returns the new page id.
  importFile: (workspaceId: string, parent: string) =>
    invoke<string | null>('page_import_file', { workspaceId, parent }),
  /// Native dialogs + import of a Notion export or Obsidian vault (folder or
  /// .zip) under `parent` ('' = top level). Null when cancelled.
  importWorkspace: (workspaceId: string, parent: string) =>
    invoke<Imported | null>('workspace_import', { workspaceId, parent }),
  /// Native folder picker + export of every page as folders of markdown.
  /// Returns the folder written; null when cancelled.
  exportWorkspace: (workspaceId: string) =>
    invoke<string | null>('workspace_export', { workspaceId }),

  /// Stores pasted image bytes; returns the `kawruh-asset://` src for the page.
  assetPut: (workspaceId: string, bytes: Uint8Array, mime: string) =>
    invoke<string>('asset_put', bytes, {
      headers: { 'x-kawruh-workspace': workspaceId, 'x-kawruh-mime': mime },
    }),
  linkPreview: (url: string) => invoke<LinkMeta>('link_preview', { url }),

  /// Imports the .md files opened with Kawruh into this workspace; their page ids.
  openPendingFiles: (workspaceId: string) => invoke<string[]>('open_pending_files', { workspaceId }),
  pendingFiles: () => invoke<number>('pending_files'),
  searchPages: (workspaceId: string, query: string) =>
    invoke<SearchHit[]>('search_pages', { workspaceId, query }),
  pageBacklinks: (workspaceId: string, pageId: string) =>
    invoke<PageRef[]>('page_backlinks', { workspaceId, pageId }),
  pageHistory: (pageId: string) => invoke<HistoryEntry[]>('page_history', { pageId }),
  pageHistoryGet: (pageId: string, id: number) => invoke<string>('page_history_get', { pageId, id }),
  pageHistoryRestore: (workspaceId: string, pageId: string, id: number) =>
    invoke<void>('page_history_restore', { workspaceId, pageId, id }),
  pagePrintData: (workspaceId: string, pageId: string, includeChildren: boolean) =>
    invoke<PrintPage[]>('page_print_data', { workspaceId, pageId, includeChildren }),
  /// Prints the prepared layout: a PDF via save dialog on macOS (its path), the
  /// system print dialog elsewhere (null).
  exportPdf: (title: string) => invoke<string | null>('page_export_pdf', { title }),

  settingGet: (key: string) => invoke<string | null>('setting_get', { key }),
  settingSet: (key: string, value: string) => invoke<void>('setting_set', { key, value }),
}
