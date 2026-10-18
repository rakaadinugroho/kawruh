/// Webview mirrors of the Rust-owned documents.
///
/// Each document id has at most one `Y.Doc` in the webview, shared by every
/// component through reference counting. It is connected to the Rust DocHub
/// as an IPC "view": local updates go up with `doc_apply`, the hub pushes
/// everyone else's updates (other views, the server, MCP agents) down a
/// channel. Awareness (cursors, presence) rides the same channel.
import { Channel } from '@tauri-apps/api/core'
import {
  Awareness,
  applyAwarenessUpdate,
  encodeAwarenessUpdate,
  removeAwarenessStates,
} from 'y-protocols/awareness'
import * as Y from 'yjs'
import { fromB64, toB64 } from './b64'
import { api, type DocEvent } from './ipc'

/// Origin for changes that came from Rust; never sent back up.
export const ORIGIN_HUB = 'hub'

export type DocHandle = {
  docId: string
  workspaceId: string
  doc: Y.Doc
  awareness: Awareness
  ready: Promise<void>
  isReady: () => boolean
}

type Entry = DocHandle & {
  refs: number
  closeTimer: ReturnType<typeof setTimeout> | null
  close: () => void
}

const entries = new Map<string, Entry>()

// Dev builds expose the open documents to the E2E automation hook.
if (import.meta.env.DEV) {
  ;(window as unknown as { __kawruhDocs: Map<string, Entry> }).__kawruhDocs = entries
}

/// Keep released docs around briefly so quick back-and-forth navigation (and
/// React StrictMode's double mount) doesn't reopen them.
const LINGER_MS = 3000

function open(docId: string, workspaceId: string): Entry {
  const doc = new Y.Doc()
  const awareness = new Awareness(doc)
  awareness.setLocalState(null)

  let view: number | null = null
  let ready = false
  let closed = false
  const inbox: DocEvent[] = []
  const outbox: Uint8Array[] = []

  const receive = (ev: DocEvent) => {
    const bytes = fromB64(ev.update)
    if (ev.type === 'update') Y.applyUpdate(doc, bytes, ORIGIN_HUB)
    else applyAwarenessUpdate(awareness, bytes, ORIGIN_HUB)
  }

  const channel = new Channel<DocEvent>()
  channel.onmessage = (ev) => {
    if (closed) return
    if (ready) receive(ev)
    else inbox.push(ev)
  }

  const send = (update: Uint8Array) => {
    if (view === null) {
      outbox.push(update)
      return
    }
    api.docApply(docId, view, toB64(update)).catch((e) => console.error('doc_apply', docId, e))
  }

  doc.on('update', (update: Uint8Array, origin: unknown) => {
    if (origin !== ORIGIN_HUB) send(update)
  })

  awareness.on(
    'update',
    (
      { added, updated, removed }: { added: number[]; updated: number[]; removed: number[] },
      origin: unknown
    ) => {
      if (origin === ORIGIN_HUB) return
      const mine = [...added, ...updated, ...removed].filter((c) => c === doc.clientID)
      if (mine.length === 0) return
      api
        .awarenessSend(workspaceId, docId, toB64(encodeAwarenessUpdate(awareness, mine)))
        .catch(() => {})
    }
  )

  const readyPromise = api.docOpen(docId, workspaceId, channel).then(({ view: v, state }) => {
    view = v
    Y.applyUpdate(doc, fromB64(state), ORIGIN_HUB)
    ready = true
    inbox.splice(0).forEach(receive)
    if (outbox.length) send(Y.mergeUpdates(outbox.splice(0)))
  })

  const entry: Entry = {
    docId,
    workspaceId,
    doc,
    awareness,
    ready: readyPromise,
    isReady: () => ready,
    refs: 0,
    closeTimer: null,
    close: () => {
      // Tell collaborators we left before tearing down.
      removeAwarenessStates(awareness, [doc.clientID], 'close')
      closed = true
      awareness.destroy()
      readyPromise
        .then(() => (view !== null ? api.docClose(docId, view) : undefined))
        .catch(() => {})
      doc.destroy()
    },
  }
  return entry
}

export function acquireDoc(docId: string, workspaceId: string): DocHandle {
  let entry = entries.get(docId)
  if (entry && entry.workspaceId !== workspaceId) {
    entry.close()
    entries.delete(docId)
    entry = undefined
  }
  if (!entry) {
    entry = open(docId, workspaceId)
    entries.set(docId, entry)
  }
  if (entry.closeTimer) {
    clearTimeout(entry.closeTimer)
    entry.closeTimer = null
  }
  entry.refs += 1
  return entry
}

export function releaseDoc(docId: string) {
  const entry = entries.get(docId)
  if (!entry) return
  entry.refs -= 1
  if (entry.refs > 0) return
  entry.closeTimer = setTimeout(() => {
    if (entry.refs > 0) return
    entry.close()
    entries.delete(docId)
  }, LINGER_MS)
}

/// Closes every open document of a workspace immediately (workspace removed).
export function closeWorkspaceDocs(workspaceId: string) {
  for (const [id, entry] of entries) {
    if (entry.workspaceId !== workspaceId) continue
    if (entry.closeTimer) clearTimeout(entry.closeTimer)
    entry.close()
    entries.delete(id)
  }
}
