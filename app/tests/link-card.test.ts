import { Editor, parserCtx, rootCtx, serializerCtx } from '@milkdown/kit/core'
import { commonmark } from '@milkdown/kit/preset/commonmark'
import { gfm } from '@milkdown/kit/preset/gfm'
import { beforeAll, expect, it } from 'vitest'
import { canonicalSrc } from '../src/editor/assets'
import { kawruhSchema } from '../src/editor/schema'

let editor: Editor

beforeAll(async () => {
  const root = document.createElement('div')
  document.body.append(root)
  editor = await Editor.make()
    .config((ctx) => ctx.set(rootCtx, root))
    .use(commonmark)
    .use(gfm)
    .use(kawruhSchema)
    .create()
})

it('link cards survive a Milkdown markdown round trip', () => {
  const parser = editor.ctx.get(parserCtx)
  const serializer = editor.ctx.get(serializerCtx)
  const md = '[Example *Domain*](https://example.com "kawruh:card")\n\n[](https://e.org/x "kawruh:preview")\n'
  const doc = parser(md)
  const [card, preview] = [doc.child(0), doc.child(1)]
  expect(card.type.name).toBe('kawruh_link')
  expect(card.attrs).toEqual({ href: 'https://example.com', label: 'Example Domain', view: 'card' })
  expect(preview.attrs).toEqual({ href: 'https://e.org/x', label: '', view: 'preview' })

  const out = serializer(doc)
  expect(out).toContain('"kawruh:card"')
  expect(parser(out).toJSON()).toEqual(doc.toJSON())
})

it('a marked link next to other text stays an inline link', () => {
  const doc = editor.ctx.get(parserCtx)('see [x](https://e.com "kawruh:card")')
  expect(doc.child(0).type.name).toBe('paragraph')
})

it('served asset URLs map back to the stored form', () => {
  expect(canonicalSrc('kawruh-asset://localhost/ws1%2Fabc.png')).toBe('kawruh-asset://ws1/abc.png')
  expect(canonicalSrc('http://kawruh-asset.localhost/ws1%2Fabc.png')).toBe('kawruh-asset://ws1/abc.png')
  expect(canonicalSrc('https://example.com/a.png')).toBe('https://example.com/a.png')
})
