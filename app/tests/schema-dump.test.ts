import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { Editor, rootCtx, schemaCtx } from '@milkdown/kit/core'
import { commonmark } from '@milkdown/kit/preset/commonmark'
import { gfm } from '@milkdown/kit/preset/gfm'
import { expect, it } from 'vitest'
import { kawruhSchema } from '../src/editor/schema'

/// Writes the ProseMirror schema the editor actually runs with to
/// crates/kawruh-core/schema.json — the contract the Rust converter is checked against.
it('dumps the editor schema contract', async () => {
  const root = document.createElement('div')
  document.body.append(root)
  const editor = await Editor.make()
    .config((ctx) => ctx.set(rootCtx, root))
    .use(commonmark)
    .use(gfm)
    .use(kawruhSchema)
    .create()
  const schema = editor.ctx.get(schemaCtx)

  const attrsOf = (spec: Record<string, any> | undefined) =>
    Object.fromEntries(
      Object.entries(spec ?? {}).map(([k, v]) => [
        k,
        {
          required: !('default' in v),
          default: v.default ?? null,
          validate: typeof v.validate === 'string' ? v.validate : null,
        },
      ])
    )

  const nodes = Object.fromEntries(
    Object.values(schema.nodes).map((t) => [
      t.name,
      {
        group: t.spec.group ?? null,
        content: t.spec.content ?? null,
        marks: t.spec.marks ?? null,
        inline: !!t.spec.inline,
        atom: !!t.spec.atom,
        code: !!t.spec.code,
        attrs: attrsOf(t.spec.attrs),
      },
    ])
  )
  const marks = Object.fromEntries(
    Object.values(schema.marks).map((t) => [
      t.name,
      {
        attrs: attrsOf(t.spec.attrs),
        excludesSelf: t.excludes(t),
      },
    ])
  )
  const out = { topNode: schema.topNodeType.name, nodes, marks }
  writeFileSync(
    resolve(__dirname, '../../crates/kawruh-core/schema.json'),
    JSON.stringify(out, null, 2) + '\n'
  )
  expect(nodes.kawruh_canvas).toBeDefined()
  await editor.destroy()
})
