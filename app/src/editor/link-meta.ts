/// Link card metadata lives in the page doc (`link_meta`: href → JSON), so
/// collaborators and the share page see it without fetching the site again.
import type * as Y from 'yjs'
import { type LinkMeta, api } from '../lib/ipc'

export const linkMetaMap = (doc: Y.Doc) => doc.getMap<string>('link_meta')

export function readMeta(doc: Y.Doc, href: string): LinkMeta | null {
  const raw = linkMetaMap(doc).get(href)
  if (!raw) return null
  try {
    return JSON.parse(raw) as LinkMeta
  } catch {
    return null
  }
}

const inflight = new Map<string, Promise<LinkMeta | null>>()

/// Metadata for `href`: from the doc, or fetched (once per URL for a while,
/// failures included). Only editors store it in the doc.
export function ensureMeta(doc: Y.Doc, href: string, canWrite: boolean): Promise<LinkMeta | null> {
  const have = readMeta(doc, href)
  if (have) return Promise.resolve(have)
  let pending = inflight.get(href)
  if (!pending) {
    pending = api.linkPreview(href).catch(() => null)
    inflight.set(href, pending)
    setTimeout(() => inflight.delete(href), 60_000)
  }
  return pending.then((meta) => {
    if (meta && canWrite && !linkMetaMap(doc).has(href)) {
      linkMetaMap(doc).set(href, JSON.stringify(meta))
    }
    return meta
  })
}

export function hostOf(href: string): string {
  try {
    return new URL(href).host.replace(/^www\./, '')
  } catch {
    return href
  }
}
