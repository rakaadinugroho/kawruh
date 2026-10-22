import { expect, it } from 'vitest'
import * as Y from 'yjs'
import {
  type Comment,
  commentThreads,
  mention,
  mentionedUsers,
  newComment,
  readComments,
  removeComment,
  replyComment,
  segments,
  writeComment,
} from '../src/lib/comments'

const author = { id: 'u1', name: 'Alice' }

it('comments round trip through the map', () => {
  const doc = new Y.Doc()
  const root = newComment('Hello', author)
  const reply = replyComment('Hi', { id: 'u2', name: 'Bob' }, root)
  writeComment(doc, root)
  writeComment(doc, reply)

  const all = readComments(doc)
  expect(all).toHaveLength(2)
  expect(all[0].body).toBe('Hello')
  expect(all[0].thread).toBe(root.id)
  expect(all[1].thread).toBe(root.id)
  expect(all[1].reply_to).toBe(root.id)
})

it('a reply never jumps above its root', () => {
  const doc = new Y.Doc()
  const root: Comment = { ...newComment('Hello', author), created: 10 }
  const reply: Comment = { ...replyComment('Hi', { id: 'u2', name: 'Bob' }, root), created: 10 }
  writeComment(doc, reply)
  writeComment(doc, root)
  const all = readComments(doc)
  expect(all[0].id).toBe(root.id)
  expect(all[1].id).toBe(reply.id)
})

it('the stored comment is the same shape the server reads', () => {
  const doc = new Y.Doc()
  const c = newComment('Hello', author)
  writeComment(doc, c)
  const raw = doc.getMap('comments').get(c.id) as string
  expect(JSON.parse(raw)).toEqual({
    id: c.id,
    thread: c.thread,
    reply_to: '',
    quote: '',
    anchor: '',
    anchor_end: '',
    body: 'Hello',
    author: 'u1',
    author_name: 'Alice',
    created: c.created,
    resolved: false,
  })
})

it('threads keep the root and its replies together', () => {
  const doc = new Y.Doc()
  const root: Comment = { ...newComment('One', author), created: 10 }
  const reply: Comment = { ...replyComment('Two', { id: 'u2', name: 'Bob' }, root), created: 20 }
  const other: Comment = { ...newComment('Three', author), created: 30 }
  writeComment(doc, root)
  writeComment(doc, reply)
  writeComment(doc, other)

  const threads = commentThreads(doc)
  expect(threads).toHaveLength(2)
  expect(threads[0]).toHaveLength(2)
  expect(threads[0][0].id).toBe(root.id)
  expect(threads[0][1].id).toBe(reply.id)
  expect(threads[1]).toHaveLength(1)
})

it('writing the same id replaces the comment', () => {
  const doc = new Y.Doc()
  const c = newComment('First', author)
  writeComment(doc, c)
  writeComment(doc, { ...c, body: 'Second', resolved: true })
  const all = readComments(doc)
  expect(all).toHaveLength(1)
  expect(all[0].body).toBe('Second')
  expect(all[0].resolved).toBe(true)
  removeComment(doc, c.id)
  expect(readComments(doc)).toHaveLength(0)
})

it('broken entries are skipped', () => {
  const doc = new Y.Doc()
  doc.getMap('comments').set('bad', 'not json')
  writeComment(doc, newComment('Good', author))
  expect(readComments(doc)).toHaveLength(1)
})

it('mentions are read from the body', () => {
  const body = 'hi @[Alice](kawruh://user/u1) and @[Bob](kawruh://user/u2)'
  expect(mentionedUsers(body)).toEqual(['u1', 'u2'])
  // The same person twice is mentioned once.
  expect(mentionedUsers('@[A](kawruh://user/u1) @[B](kawruh://user/u1)')).toEqual(['u1'])
  // A longer id that starts with another one is a different user.
  expect(mentionedUsers('@[A](kawruh://user/u1) @[B](kawruh://user/u12)')).toEqual(['u1', 'u12'])
  expect(mentionedUsers('no mentions here')).toEqual([])
})

it('a mention is a link to a user, and reads back as one', () => {
  const text = mention('u7', 'Robin')
  expect(text).toBe('@[Robin](kawruh://user/u7)')
  expect(mentionedUsers(text)).toEqual(['u7'])
  expect(segments(text)).toEqual([{ text: '@Robin', userId: 'u7' }])
})

it('a comment body reads as text with its mentions marked', () => {
  const body = 'look at @[Robin](kawruh://user/u7) please'
  expect(segments(body)).toEqual([
    { text: 'look at ' },
    { text: '@Robin', userId: 'u7' },
    { text: ' please' },
  ])
})
