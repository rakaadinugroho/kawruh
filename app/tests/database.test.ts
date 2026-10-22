import { expect, it } from 'vitest'
import * as Y from 'yjs'
import {
  type Property,
  addRow,
  cellText,
  cellValue,
  defaultSchema,
  fromBlock,
  fromJson,
  readRows,
  readSchema,
  readView,
  removeRow,
  rowTitle,
  toBlock,
  toJson,
  toMarkdown,
  writeCell,
  writeSchema,
  writeView,
} from '../src/lib/database'

const columns: Property[] = defaultSchema()

it('the schema round trips', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  expect(readSchema(doc)).toEqual(columns)
  // The status column knows its choices.
  expect(readSchema(doc)[1].options).toEqual(['Todo', 'Doing', 'Done'])
})

it('a page without a database has no columns', () => {
  const doc = new Y.Doc()
  expect(readSchema(doc)).toEqual([])
  expect(readRows(doc)).toEqual([])
  expect(toJson(doc)).toBeNull()
})

it('rows are read back', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const id = addRow(doc)
  writeCell(doc, id, 'name', 'Write the docs')
  writeCell(doc, id, 'status', 'Doing')

  const rows = readRows(doc)
  expect(rows).toHaveLength(1)
  expect(rows[0].id).toBe(id)
  expect(cellText(cellValue(rows[0], 'name'))).toBe('Write the docs')
  expect(cellText(cellValue(rows[0], 'status'))).toBe('Doing')
  // An empty cell is empty, not missing.
  expect(cellText(cellValue(rows[0], 'due'))).toBe('')
})

it('one cell is written without touching the others', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const id = addRow(doc)
  writeCell(doc, id, 'name', 'Write the docs')
  writeCell(doc, id, 'status', 'Doing')
  writeCell(doc, id, 'name', 'Write the docs, again')

  const rows = readRows(doc)
  expect(cellText(cellValue(rows[0], 'name'))).toBe('Write the docs, again')
  expect(cellText(cellValue(rows[0], 'status'))).toBe('Doing')
})

it('a new row lands at the end', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const a = addRow(doc)
  const b = addRow(doc)
  const rows = readRows(doc)
  expect(rows.map((r) => r.id)).toEqual([a, b])
})

it('a removed row is gone', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const id = addRow(doc)
  removeRow(doc, id)
  expect(readRows(doc)).toHaveLength(0)
})

it('the view is remembered', () => {
  const doc = new Y.Doc()
  expect(readView(doc)).toBe('table')
  writeView(doc, 'board')
  expect(readView(doc)).toBe('board')
})

it('a database survives a json round trip', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const id = addRow(doc)
  writeCell(doc, id, 'name', 'Write the docs')
  const json = toJson(doc) as string
  const db = fromJson(json)
  expect(db?.columns).toHaveLength(3)
  expect(db?.rows).toHaveLength(1)
  expect(db?.rows[0].cells.name).toBe('Write the docs')
})

it('a database survives a block round trip', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const id = addRow(doc)
  writeCell(doc, id, 'name', 'Write the docs')
  const block = toBlock(doc) as string
  expect(block.startsWith('```kawruh-database')).toBe(true)
  const db = fromBlock(block)
  expect(db?.rows[0].cells.name).toBe('Write the docs')
  // A block inside a longer page is found too.
  expect(fromBlock(`Some text\n\n${block}\n\nMore text\n`)).not.toBeNull()
})

it('a database reads as a markdown table', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const id = addRow(doc)
  writeCell(doc, id, 'name', 'Write the docs')
  writeCell(doc, id, 'status', 'Doing')
  const table = toMarkdown(readSchema(doc), readRows(doc))
  expect(table).toContain('| Name | Status | Due |')
  expect(table).toContain('| Write the docs | Doing |  |')
})

it('a pipe in a value does not break the table', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const id = addRow(doc)
  writeCell(doc, id, 'name', 'a | b')
  const table = toMarkdown(readSchema(doc), readRows(doc))
  expect(table).toContain('a \\| b')
})

it('a row is named after its title column', () => {
  const doc = new Y.Doc()
  writeSchema(doc, columns)
  const id = addRow(doc)
  writeCell(doc, id, 'name', 'Write the docs')
  const rows = readRows(doc)
  expect(rowTitle(rows[0], readSchema(doc))).toBe('Write the docs')
})
