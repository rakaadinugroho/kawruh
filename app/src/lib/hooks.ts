import { useEffect, useMemo, useState, useSyncExternalStore } from 'react'
import { type DocHandle, acquireDoc, releaseDoc } from './docs'
import { type Page, livePages, readPages, workspaceName, pagesMap, metaMap } from './workspace'

/// Holds a shared document handle for the component's lifetime.
export function useDoc(docId: string | null, workspaceId: string | null): DocHandle | null {
  const [handle, setHandle] = useState<DocHandle | null>(null)
  const [, setTick] = useState(0)
  useEffect(() => {
    if (!docId || !workspaceId) {
      setHandle(null)
      return
    }
    const h = acquireDoc(docId, workspaceId)
    setHandle(h)
    let alive = true
    h.ready.then(() => alive && setTick((t) => t + 1)).catch(() => {})
    return () => {
      alive = false
      releaseDoc(docId)
    }
  }, [docId, workspaceId])
  return handle && handle.docId === docId ? handle : null
}

export type WorkspaceView = {
  all: Page[]
  live: Page[]
  trash: Page[]
  name: string
}

/// Live view of a workspace doc's tree and name.
export function useWorkspaceView(handle: DocHandle | null): WorkspaceView {
  const [version, setVersion] = useState(0)
  useEffect(() => {
    if (!handle) return
    const bump = () => setVersion((v) => v + 1)
    const pages = pagesMap(handle.doc)
    const meta = metaMap(handle.doc)
    pages.observeDeep(bump)
    meta.observe(bump)
    handle.ready.then(bump).catch(() => {})
    return () => {
      pages.unobserveDeep(bump)
      meta.unobserve(bump)
    }
  }, [handle])
  return useMemo(() => {
    if (!handle) return { all: [], live: [], trash: [], name: '' }
    const all = readPages(handle.doc)
    const live = livePages(all)
    return {
      all,
      live,
      trash: all.filter((p) => p.deleted !== 0).sort((a, b) => b.deleted - a.deleted),
      name: workspaceName(handle.doc),
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [handle, version])
}

export type Peer = { clientId: number; name: string; color: string }

/// Other people present in a document (from awareness).
export function usePeers(handle: DocHandle | null): Peer[] {
  const subscribe = useMemo(
    () => (cb: () => void) => {
      if (!handle) return () => {}
      handle.awareness.on('change', cb)
      return () => handle.awareness.off('change', cb)
    },
    [handle]
  )
  const snapshot = useMemo(() => {
    let cache: { key: string; peers: Peer[] } = { key: '', peers: [] }
    return () => {
      if (!handle) return cache.peers
      const peers: Peer[] = []
      handle.awareness.getStates().forEach((state, clientId) => {
        if (clientId === handle.doc.clientID) return
        const user = (state as { user?: { name?: string; color?: string } }).user
        if (user?.name) peers.push({ clientId, name: user.name, color: user.color ?? '#888' })
      })
      const key = peers.map((p) => `${p.clientId}:${p.name}`).join('|')
      if (key !== cache.key) cache = { key, peers }
      return cache.peers
    }
  }, [handle])
  return useSyncExternalStore(subscribe, snapshot)
}

/// Stable per-person color (amber-to-ink family, readable on paper and ink).
export function colorFor(seed: string): string {
  let h = 0
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0
  const hues = [28, 62, 145, 200, 262, 320]
  return `oklch(0.62 0.13 ${hues[h % hues.length]})`
}
