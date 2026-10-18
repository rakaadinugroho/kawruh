/// Page tree operations on the workspace Y.Doc.
/// Mirrors crates/kawruh-core/src/workspace.rs (same layout, same rules):
/// `meta: Map{ name }`, `pages: Map<id, Map{ title, parent, order, icon, created, deleted }>`.
import * as Y from 'yjs'

export type Page = {
  id: string
  title: string
  parent: string
  order: number
  icon: string
  created: number
  deleted: number
}

export const pagesMap = (doc: Y.Doc) => doc.getMap<Y.Map<unknown>>('pages')
export const metaMap = (doc: Y.Doc) => doc.getMap<unknown>('meta')

export const newId = () => crypto.randomUUID().replaceAll('-', '')

export function readPages(doc: Y.Doc): Page[] {
  const out: Page[] = []
  pagesMap(doc).forEach((m, id) => {
    if (!(m instanceof Y.Map)) return
    out.push({
      id,
      title: String(m.get('title') ?? ''),
      parent: String(m.get('parent') ?? ''),
      order: Number(m.get('order') ?? 0),
      icon: String(m.get('icon') ?? ''),
      created: Number(m.get('created') ?? 0),
      deleted: Number(m.get('deleted') ?? 0),
    })
  })
  return breakCycles(out.sort((a, b) => a.order - b.order || (a.id < b.id ? -1 : 1)))
}

/// Concurrent moves can merge into a parent loop (X under Y, Y under X), which
/// would hide every page in it. The smallest id of each loop is shown at the
/// root instead. Mirrors `break_cycles` in kawruh-core.
export function breakCycles(pages: Page[]): Page[] {
  const byId = new Map(pages.map((p) => [p.id, p]))
  const breakers = new Set<string>()
  for (const start of pages) {
    const path: string[] = []
    let cur: Page | undefined = start
    while (cur && !breakers.has(cur.id)) {
      const pos = path.indexOf(cur.id)
      if (pos >= 0) {
        breakers.add(path.slice(pos).sort()[0])
        break
      }
      path.push(cur.id)
      cur = byId.get(cur.parent)
    }
  }
  return breakers.size === 0 ? pages : pages.map((p) => (breakers.has(p.id) ? { ...p, parent: '' } : p))
}

/// Pages not deleted and not under a deleted (or cyclic) ancestor. Orphans
/// whose parent hasn't synced yet show at the root.
export function livePages(all: Page[]): Page[] {
  const byId = new Map(all.map((p) => [p.id, p]))
  const isLive = (p: Page) => {
    const seen = new Set<string>()
    let cur: Page | undefined = p
    while (cur) {
      if (cur.deleted !== 0 || seen.has(cur.id)) return false
      seen.add(cur.id)
      if (!cur.parent) return true
      const parent = byId.get(cur.parent)
      if (!parent) return true
      cur = parent
    }
    return true
  }
  return all.filter(isLive)
}

/// Children of `parent` among live pages ("" = root, including orphans).
export function childrenOf(live: Page[], parent: string): Page[] {
  if (parent) return live.filter((p) => p.parent === parent)
  const ids = new Set(live.map((p) => p.id))
  return live.filter((p) => !p.parent || !ids.has(p.parent))
}

export function pagePath(all: Page[], id: string): Page[] {
  const byId = new Map(all.map((p) => [p.id, p]))
  const out: Page[] = []
  const seen = new Set<string>()
  let cur = byId.get(id)
  while (cur && !seen.has(cur.id)) {
    out.unshift(cur)
    seen.add(cur.id)
    cur = byId.get(cur.parent)
  }
  return out
}

function orderAt(orders: number[], index?: number): number {
  const i = Math.min(index ?? orders.length, orders.length)
  const prev = i > 0 ? orders[i - 1] : undefined
  const next = orders[i]
  if (prev === undefined && next === undefined) return 1
  if (prev === undefined) return next! - 1
  if (next === undefined) return prev + 1
  return (prev + next) / 2
}

function siblingOrders(doc: Y.Doc, parent: string, except: string): number[] {
  return readPages(doc)
    .filter((p) => p.parent === parent && p.id !== except && p.deleted === 0)
    .map((p) => p.order)
    .sort((a, b) => a - b)
}

export function createPage(doc: Y.Doc, opts: { title?: string; parent?: string } = {}): string {
  const id = newId()
  const parent = opts.parent ?? ''
  const order = orderAt(siblingOrders(doc, parent, id))
  doc.transact(() => {
    const m = new Y.Map<unknown>()
    m.set('title', opts.title ?? '')
    m.set('parent', parent)
    m.set('order', order)
    m.set('icon', '')
    m.set('created', Date.now())
    m.set('deleted', 0)
    pagesMap(doc).set(id, m)
  })
  return id
}

function setField(doc: Y.Doc, id: string, key: string, value: unknown) {
  const m = pagesMap(doc).get(id)
  if (m instanceof Y.Map) m.set(key, value)
}

export const renamePage = (doc: Y.Doc, id: string, title: string) =>
  setField(doc, id, 'title', title)
export const setPageIcon = (doc: Y.Doc, id: string, icon: string) => setField(doc, id, 'icon', icon)
export const deletePage = (doc: Y.Doc, id: string) => setField(doc, id, 'deleted', Date.now())
export const restorePage = (doc: Y.Doc, id: string) => setField(doc, id, 'deleted', 0)
/// Deletes a page and its whole subtree for good (sub-pages would otherwise
/// lose their parent and resurface at the root).
export function purgePage(doc: Y.Doc, id: string) {
  const all = readPages(doc)
  const doomed = all.filter((p) => isWithin(all, id, p.id)).map((p) => p.id)
  doc.transact(() => doomed.forEach((d) => pagesMap(doc).delete(d)))
}

/// True when `candidate` is `id` or one of its descendants.
export function isWithin(all: Page[], id: string, candidate: string): boolean {
  const byId = new Map(all.map((p) => [p.id, p]))
  const seen = new Set<string>()
  let cur: string | undefined = candidate
  while (cur && !seen.has(cur)) {
    if (cur === id) return true
    seen.add(cur)
    cur = byId.get(cur)?.parent
  }
  return false
}

/// Moves a page under `parent` at sibling `index` (end when omitted).
/// Returns false (and changes nothing) for a move into its own subtree.
export function movePage(doc: Y.Doc, id: string, parent: string, index?: number): boolean {
  if (parent && isWithin(readPages(doc), id, parent)) return false
  const order = orderAt(siblingOrders(doc, parent, id), index)
  doc.transact(() => {
    setField(doc, id, 'parent', parent)
    setField(doc, id, 'order', order)
  })
  return true
}

export const workspaceName = (doc: Y.Doc) => String(metaMap(doc).get('name') ?? '')
export const setWorkspaceName = (doc: Y.Doc, name: string) => metaMap(doc).set('name', name)
