/// Database pages: a table of rows with typed properties.
/// Mirrors crates/kawruh-core/src/database.rs: the same layout (`schema` and
/// `view` are root maps holding one value, `rows` is a map of row id → map of
/// property id → value) and the same field names, so a database written here
/// is the same database the server and MCP agents see.
import * as Y from 'yjs'

export const ROWS = 'rows'
export const SCHEMA = 'schema'
export const VIEW = 'view'
/// The key a row's position is kept under; property ids never start with `_`.
export const ORDER_KEY = '__order'
/// The fenced-code language a database is written as.
export const DATABASE_LANG = 'kawruh-database'
/// The page kind of a database page.
export const KIND_DATABASE = 'database'

export const KINDS = [
  'title',
  'text',
  'number',
  'select',
  'multi_select',
  'date',
  'checkbox',
  'person',
  'url',
] as const
export type PropertyKind = (typeof KINDS)[number]

/// One column of a database.
export type Property = {
  id: string
  name: string
  type: PropertyKind
  options?: string[]
}

/// A value in a cell.
export type CellValue = string | number | boolean | string[]

/// One row of a database.
export type Row = {
  id: string
  cells: Record<string, CellValue>
  order: number
}

/// The database as it is written to JSON.
export type DatabaseJson = {
  columns: Property[]
  rows: { id: string; cells: Record<string, CellValue> }[]
}

export const schemaMap = (doc: Y.Doc) => doc.getMap<string>(SCHEMA)
export const viewMap = (doc: Y.Doc) => doc.getMap<string>(VIEW)
export const rowsMap = (doc: Y.Doc) => doc.getMap<Y.Map<unknown>>(ROWS)

/// The properties of a database page, in order.
export function readSchema(doc: Y.Doc): Property[] {
  const raw = schemaMap(doc).get('value')
  if (!raw) return []
  try {
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed.filter((p): p is Property => {
      const prop = p as Property
      return typeof prop?.id === 'string' && typeof prop?.name === 'string'
    })
  } catch {
    return []
  }
}

/// Writes the properties of a database page.
export function writeSchema(doc: Y.Doc, properties: Property[]) {
  schemaMap(doc).set('value', JSON.stringify(properties))
}

/// The view a database opens in.
export function readView(doc: Y.Doc): 'table' | 'board' {
  return viewMap(doc).get('value') === 'board' ? 'board' : 'table'
}

/// Sets the view a database opens in.
export function writeView(doc: Y.Doc, view: 'table' | 'board') {
  viewMap(doc).set('value', view)
}

function readCell(value: unknown): CellValue {
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
    return value
  }
  if (value instanceof Y.Array) return value.toArray().map(String)
  if (Array.isArray(value)) return value.map(String)
  return ''
}

/// The rows of a database page, in the order they were created.
export function readRows(doc: Y.Doc): Row[] {
  const out: Row[] = []
  rowsMap(doc).forEach((m, id) => {
    if (!(m instanceof Y.Map)) return
    const cells: Record<string, CellValue> = {}
    let order = 0
    m.forEach((value, key) => {
      if (key === ORDER_KEY) {
        order = Number(value) || 0
        return
      }
      cells[key] = readCell(value)
    })
    out.push({ id, cells, order })
  })
  return out.sort((a, b) => a.order - b.order || (a.id < b.id ? -1 : 1))
}

/// The value of one cell, or an empty string.
export function cellValue(row: Row, property: string): CellValue {
  return row.cells[property] ?? ''
}

/// Writes one cell of a row, leaving the rest of the row alone.
export function writeCell(doc: Y.Doc, rowId: string, property: string, value: CellValue) {
  const row = rowsMap(doc).get(rowId)
  if (!(row instanceof Y.Map)) return
  row.set(property, value)
}

/// Adds an empty row at the end and returns its id.
export function addRow(doc: Y.Doc): string {
  const id = crypto.randomUUID().replaceAll('-', '')
  const order = readRows(doc).reduce((max, r) => Math.max(max, r.order), 0) + 1
  doc.transact(() => {
    const row = new Y.Map<unknown>()
    row.set(ORDER_KEY, order)
    rowsMap(doc).set(id, row)
  })
  return id
}

/// Removes a row.
export function removeRow(doc: Y.Doc, rowId: string) {
  rowsMap(doc).delete(rowId)
}

/// A new database's columns: a name, a status and a date.
export function defaultSchema(): Property[] {
  return [
    { id: 'name', name: 'Name', type: 'title' },
    { id: 'status', name: 'Status', type: 'select', options: ['Todo', 'Doing', 'Done'] },
    { id: 'due', name: 'Due', type: 'date' },
  ]
}

/// The database of a page, as JSON; null when the page has no database.
export function toJson(doc: Y.Doc): string | null {
  const columns = readSchema(doc)
  if (columns.length === 0) return null
  const rows = readRows(doc).map((r) => ({ id: r.id, cells: r.cells }))
  return JSON.stringify({ columns, rows })
}

/// A database read from JSON.
export function fromJson(raw: string): DatabaseJson | null {
  try {
    const parsed = JSON.parse(raw) as DatabaseJson
    if (!parsed || !Array.isArray(parsed.columns) || !Array.isArray(parsed.rows)) return null
    return parsed
  } catch {
    return null
  }
}

/// The database of a page as a `kawruh-database` code fence.
export function toBlock(doc: Y.Doc): string | null {
  const json = toJson(doc)
  return json === null ? null : `\`\`\`${DATABASE_LANG}\n${json}\n\`\`\``
}

/// A database read from a `kawruh-database` code fence.
export function fromBlock(markdown: string): DatabaseJson | null {
  const fence = `\`\`\`${DATABASE_LANG}`
  const start = markdown.indexOf(fence)
  if (start < 0) return null
  const after = markdown.slice(start + fence.length)
  const end = after.indexOf('```')
  if (end < 0) return null
  return fromJson(after.slice(0, end).trim())
}

/// A cell value as plain text.
export function cellText(value: CellValue | undefined): string {
  if (value === undefined) return ''
  if (typeof value === 'string') return value
  if (typeof value === 'number') return String(value)
  if (typeof value === 'boolean') return value ? 'yes' : ''
  return value.join(', ')
}

/// Keeps a `|` or a newline from breaking a markdown table.
function escape(text: string): string {
  return text.replaceAll('|', '\\|').replaceAll('\n', ' ')
}

/// A database as a markdown table.
export function toMarkdown(columns: Property[], rows: Row[]): string {
  if (columns.length === 0) return ''
  const lines: string[] = []
  lines.push(`| ${columns.map((c) => escape(c.name)).join(' | ')} |`)
  lines.push(`| ${columns.map(() => '---').join(' | ')} |`)
  for (const row of rows) {
    const cells = columns.map((c) => escape(cellText(cellValue(row, c.id))))
    lines.push(`| ${cells.join(' | ')} |`)
  }
  return lines.join('\n')
}

/// The row's title: the value of the first `title` column.
export function rowTitle(row: Row, columns: Property[]): string {
  const title = columns.find((c) => c.type === 'title')
  return title ? cellText(cellValue(row, title.id)) : ''
}
