/// Comments on a page, read from the page document.
/// Mirrors crates/kawruh-core/src/comments.rs: the same layout (`comments` is a
/// Y.Map of comment id → JSON string) and the same field names, so a comment
/// written here is the same comment the server and MCP agents see.
import * as Y from 'yjs'

export const COMMENTS = 'comments'
/// The link prefix that mentions a user.
export const USER_LINK = 'kawruh://user/'

export type Comment = {
  id: string
  /// The root comment of the thread; a root comment carries its own id.
  thread: string
  /// The comment this one answers; empty for a root comment.
  reply_to: string
  /// The text the comment is about; empty for a comment on the page itself.
  quote: string
  /// Where the quoted text starts, as an encoded Yjs relative position.
  anchor: string
  /// Where the quoted text ends, as an encoded Yjs relative position.
  anchor_end: string
  /// The comment itself, as markdown.
  body: string
  /// The author's user id.
  author: string
  /// The author's display name, so the comment reads well without a lookup.
  author_name: string
  /// When the comment was written (ms since the epoch).
  created: number
  resolved: boolean
}

export const commentsMap = (doc: Y.Doc) => doc.getMap<string>(COMMENTS)

/// Fills in the fields the writer left out (older comments, hand-written JSON).
function normalize(raw: Partial<Comment>, id: string): Comment {
  return {
    id: String(raw.id ?? id),
    thread: String(raw.thread ?? raw.id ?? id),
    reply_to: String(raw.reply_to ?? ''),
    quote: String(raw.quote ?? ''),
    anchor: String(raw.anchor ?? ''),
    anchor_end: String(raw.anchor_end ?? ''),
    body: String(raw.body ?? ''),
    author: String(raw.author ?? ''),
    author_name: String(raw.author_name ?? ''),
    created: Number(raw.created ?? 0),
    resolved: Boolean(raw.resolved ?? false),
  }
}

/// Every comment of a page, oldest first. Entries that aren't JSON are skipped
/// rather than throwing the whole list away.
export function readComments(doc: Y.Doc): Comment[] {
  const out: Comment[] = []
  commentsMap(doc).forEach((json, id) => {
    try {
      out.push(normalize(JSON.parse(json), id))
    } catch {
      /* not one of ours */
    }
  })
  return out.sort(byAge)
}

/// Oldest first, and a root comment before the replies to it: two comments
/// written in the same millisecond would otherwise be ordered by their random
/// ids, and a reply could end up above the comment it answers.
function byAge(a: Comment, b: Comment): number {
  if (a.created !== b.created) return a.created - b.created
  const aRoot = !a.thread || a.thread === a.id ? 0 : 1
  const bRoot = !b.thread || b.thread === b.id ? 0 : 1
  if (aRoot !== bRoot) return aRoot - bRoot
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0
}

/// The comments of a page grouped into threads: the root comment first, then
/// its replies in order.
export function commentThreads(doc: Y.Doc): Comment[][] {
  const all = readComments(doc)
  const roots = all.filter((c) => !c.thread || c.thread === c.id)
  const replies = all.filter((c) => c.thread && c.thread !== c.id)
  return roots.map((root) => [root, ...replies.filter((r) => r.thread === root.id)])
}

/// Writes a comment, replacing the one with the same id.
export function writeComment(doc: Y.Doc, comment: Comment) {
  commentsMap(doc).set(comment.id, JSON.stringify(comment))
}

/// Removes a comment.
export const removeComment = (doc: Y.Doc, id: string) => commentsMap(doc).delete(id)

/// A new root comment on a page.
export function newComment(
  body: string,
  author: { id: string; name: string },
  opts: { quote?: string; anchor?: string; anchorEnd?: string } = {}
): Comment {
  const id = crypto.randomUUID().replaceAll('-', '')
  return {
    id,
    thread: id,
    reply_to: '',
    quote: opts.quote ?? '',
    anchor: opts.anchor ?? '',
    anchor_end: opts.anchorEnd ?? '',
    body,
    author: author.id,
    author_name: author.name,
    created: Date.now(),
    resolved: false,
  }
}

/// A reply in the thread of `root`.
export function replyComment(
  body: string,
  author: { id: string; name: string },
  parent: Comment
): Comment {
  const id = crypto.randomUUID().replaceAll('-', '')
  return {
    ...newComment(body, author),
    id,
    thread: parent.thread,
    reply_to: parent.id,
  }
}

/// The user ids mentioned in `body`.
export function mentionedUsers(body: string): string[] {
  const out: string[] = []
  let at = body.indexOf(USER_LINK)
  while (at >= 0) {
    const rest = body.slice(at + USER_LINK.length)
    const end = rest.search(/[^A-Za-z0-9_-]/)
    const id = end < 0 ? rest : rest.slice(0, end)
    if (id && !out.includes(id)) out.push(id)
    at = body.indexOf(USER_LINK, at + USER_LINK.length)
  }
  return out
}

/// The markdown that mentions someone: an ordinary link to a user.
export const mention = (id: string, name: string) => `@[${name}](${USER_LINK}${id})`

/// Splits a comment body into plain text and mentions, so the UI can style the
/// mentions instead of showing their markdown.
export type Segment = { text: string; userId?: string }

export function segments(body: string): Segment[] {
  const pattern = new RegExp(`@\\[([^\\]]*)\\]\\(${USER_LINK}([^)]+)\\)`, 'g')
  const out: Segment[] = []
  let at = 0
  for (const match of body.matchAll(pattern)) {
    const [whole, name, userId] = match
    if (match.index > at) out.push({ text: body.slice(at, match.index) })
    out.push({ text: `@${name || userId}`, userId })
    at = match.index + whole.length
  }
  if (at < body.length) out.push({ text: body.slice(at) })
  return out
}