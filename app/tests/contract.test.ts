import { existsSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import {
  Editor,
  parserCtx,
  rootCtx,
  schemaCtx,
  serializerCtx,
} from '@milkdown/kit/core'
import { commonmark } from '@milkdown/kit/preset/commonmark'
import { gfm } from '@milkdown/kit/preset/gfm'
import type { Node } from '@milkdown/kit/prose/model'
import { prosemirrorToYDoc, yXmlFragmentToProseMirrorRootNode } from 'y-prosemirror'
import * as Y from 'yjs'
import { beforeAll, describe, expect, it } from 'vitest'
import { kawruhSchema } from '../src/editor/schema'

/// Cross-language contract between the Rust converter (kawruh-core) and the
/// editor. Run `cargo test -p kawruh-core` first: it writes `out/*.rs.bin`.
const fixtures = resolve(__dirname, '../../crates/kawruh-core/tests/fixtures')
const out = resolve(fixtures, 'out')

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

/// Heading ids are derived by the editor at render time; ignore them. A null
/// table alignment is not stored by y-prosemirror and reads back as 'left'.
const normalize = (node: Node) =>
  JSON.parse(
    JSON.stringify(node.toJSON(), (key, value) => {
      if (key === 'id') return undefined
      if (key === 'alignment' && value === null) return 'left'
      return value
    })
  )

const names = readdirSync(fixtures)
  .filter((f) => f.endsWith('.md'))
  .map((f) => f.slice(0, -3))

describe('Rust → editor', () => {
  for (const name of names) {
    it(`${name}: Rust Yjs is accepted by y-prosemirror unchanged`, () => {
      const bin = resolve(out, `${name}.rs.bin`)
      expect(existsSync(bin), `run cargo test -p kawruh-core first (${bin})`).toBe(true)
      const schema = editor.ctx.get(schemaCtx)
      const doc = new Y.Doc()
      Y.applyUpdate(doc, readFileSync(bin))

      // y-prosemirror deletes any element it cannot turn into a node; that
      // shows up as a doc update during conversion.
      let mutated = false
      doc.on('update', () => {
        mutated = true
      })
      const pm = yXmlFragmentToProseMirrorRootNode(doc.getXmlFragment('prosemirror'), schema)
      expect(mutated, 'y-prosemirror rejected (deleted) part of the Rust document').toBe(false)
      expect(() => pm.check()).not.toThrow()

      // Rust's markdown must mean the same document to Milkdown's parser.
      const parser = editor.ctx.get(parserCtx)
      const rustMd = readFileSync(resolve(out, `${name}.rs.md`), 'utf8')
      expect(normalize(parser(rustMd))).toEqual(normalize(pm))
    })
  }
})

describe('editor → Rust', () => {
  for (const name of names) {
    it(`${name}: writes Milkdown-produced Yjs for the Rust side`, () => {
      const parser = editor.ctx.get(parserCtx)
      const serializer = editor.ctx.get(serializerCtx)
      const md = readFileSync(resolve(fixtures, `${name}.md`), 'utf8')
      const pm = parser(md)
      expect(() => pm.check()).not.toThrow()
      expect(serializer(pm).length).toBeGreaterThan(0)
      const ydoc = prosemirrorToYDoc(pm, 'prosemirror')
      writeFileSync(resolve(out, `${name}.js.bin`), Y.encodeStateAsUpdate(ydoc))
    })
  }
})
