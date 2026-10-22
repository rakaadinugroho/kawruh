/// A database page: a table of rows with typed properties, and a board view
/// that groups the rows by their first `select` column.
import { Plus, Trash2, Type as TypeIcon, LayoutGrid, Table2, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import * as Y from 'yjs'
import {
  type CellValue,
  type Property,
  type PropertyKind,
  type Row,
  KINDS,
  addRow,
  cellText,
  cellValue,
  readRows,
  readSchema,
  readView,
  removeRow,
  rowsMap,
  schemaMap,
  viewMap,
  writeCell,
  writeSchema,
  writeView,
} from '../lib/database'
import { Button, IconButton, Menu, useMenuAnchor } from './ui'

const KIND_LABELS: Record<PropertyKind, string> = {
  title: 'Title',
  text: 'Text',
  number: 'Number',
  select: 'Select',
  multi_select: 'Multi-select',
  date: 'Date',
  checkbox: 'Checkbox',
  person: 'Person',
  url: 'Link',
}

const newId = () => crypto.randomUUID().replaceAll('-', '')

/// A text-ish cell: the value is written when the field is left or Enter is
/// pressed, so typing never rewrites the document.
function TextCell({
  value,
  placeholder,
  type,
  readonly,
  onCommit,
}: {
  value: string
  placeholder?: string
  type?: string
  readonly: boolean
  onCommit: (v: string) => void
}) {
  const [text, setText] = useState(value)
  useEffect(() => setText(value), [value])
  return (
    <input
      type={type ?? 'text'}
      value={text}
      readOnly={readonly}
      placeholder={placeholder ?? ''}
      aria-label={placeholder}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => text !== value && onCommit(text)}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur()
        if (e.key === 'Escape') {
          setText(value)
          e.currentTarget.blur()
        }
      }}
      className="w-full min-w-0 bg-transparent px-2 py-1.5 text-[13px] text-ink outline-none placeholder:text-ink-3/70 focus:bg-hover-2"
    />
  )
}

/// A cell of one kind, edited in place.
function CellEditor({
  doc,
  row,
  property,
  readonly,
}: {
  doc: Y.Doc
  row: Row
  property: Property
  readonly: boolean
}) {
  const value = cellValue(row, property.id)
  const set = (v: CellValue) => writeCell(doc, row.id, property.id, v)

  if (property.type === 'checkbox') {
    return (
      <span className="flex justify-center px-2 py-1.5">
        <input
          type="checkbox"
          checked={value === true}
          readOnly={readonly}
          aria-label={property.name}
          onChange={(e) => set(e.target.checked)}
          className="size-3.5 accent-[var(--accent)]"
        />
      </span>
    )
  }

  if (property.type === 'number') {
    return (
      <TextCell
        type="number"
        value={typeof value === 'number' ? String(value) : ''}
        placeholder={property.name}
        readonly={readonly}
        onCommit={(v) => set(v === '' ? 0 : Number(v))}
      />
    )
  }

  if (property.type === 'date') {
    return (
      <TextCell
        type="date"
        value={typeof value === 'string' ? value : ''}
        placeholder={property.name}
        readonly={readonly}
        onCommit={(v) => set(v)}
      />
    )
  }

  if (property.type === 'select') {
    return (
      <SelectCell
        doc={doc}
        row={row}
        property={property}
        readonly={readonly}
        value={typeof value === 'string' ? value : ''}
      />
    )
  }

  if (property.type === 'multi_select') {
    return (
      <MultiSelectCell
        doc={doc}
        row={row}
        property={property}
        readonly={readonly}
        value={Array.isArray(value) ? value : []}
      />
    )
  }

  return (
    <TextCell
      value={typeof value === 'string' ? value : cellText(value)}
      placeholder={property.name}
      readonly={readonly}
      onCommit={(v) => set(v)}
    />
  )
}

/// A `select` cell: a choice of the column, with the option list editable.
function SelectCell({
  doc,
  row,
  property,
  readonly,
  value,
}: {
  doc: Y.Doc
  row: Row
  property: Property
  readonly: boolean
  value: string
}) {
  const [adding, setAdding] = useState(false)
  const [choice, setChoice] = useState('')
  const options = property.options ?? []

  const addOption = (name: string) => {
    const trimmed = name.trim()
    if (!trimmed) return
    if (!options.includes(trimmed)) {
      writeSchema(
        doc,
        readSchema(doc).map((p) =>
          p.id === property.id ? { ...p, options: [...options, trimmed] } : p
        )
      )
    }
    writeCell(doc, row.id, property.id, trimmed)
    setAdding(false)
    setChoice('')
  }

  if (adding) {
    return (
      <input
        autoFocus
        value={choice}
        aria-label={`New ${property.name} option`}
        onChange={(e) => setChoice(e.target.value)}
        onBlur={() => addOption(choice)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') addOption(choice)
          if (e.key === 'Escape') {
            setAdding(false)
            setChoice('')
          }
        }}
        className="w-full min-w-0 bg-transparent px-2 py-1.5 text-[13px] text-ink outline-none"
      />
    )
  }

  return (
    <select
      value={value}
      disabled={readonly}
      aria-label={property.name}
      onChange={(e) => {
        if (e.target.value === '__new__') {
          setAdding(true)
          return
        }
        writeCell(doc, row.id, property.id, e.target.value)
      }}
      className="w-full min-w-0 appearance-none bg-transparent px-2 py-1.5 text-[13px] text-ink outline-none disabled:opacity-60"
    >
      <option value="">—</option>
      {options.map((o) => (
        <option key={o} value={o}>
          {o}
        </option>
      ))}
      {!readonly && <option value="__new__">+ New option…</option>}
    </select>
  )
}

/// A `multi_select` cell: the chosen values, and a way to add more.
function MultiSelectCell({
  doc,
  row,
  property,
  readonly,
  value,
}: {
  doc: Y.Doc
  row: Row
  property: Property
  readonly: boolean
  value: string[]
}) {
  const options = property.options ?? []
  const set = (items: string[]) => writeCell(doc, row.id, property.id, items)
  return (
    <span className="flex flex-wrap items-center gap-1 px-2 py-1.5">
      {value.map((v) => (
        <span
          key={v}
          className="flex items-center gap-1 rounded-[4px] bg-hover-2 px-1.5 py-0.5 text-[12px] text-ink-2"
        >
          {v}
          {!readonly && (
            <button
              type="button"
              aria-label={`Remove ${v}`}
              onClick={() => set(value.filter((x) => x !== v))}
              className="press text-ink-3 hover:text-danger"
            >
              <X size={10} />
            </button>
          )}
        </span>
      ))}
      {!readonly && (
        <select
          value=""
          aria-label={`Add ${property.name}`}
          onChange={(e) => {
            if (!e.target.value) return
            set([...value, e.target.value])
          }}
          className="w-16 bg-transparent text-[12px] text-ink-3 outline-none"
        >
          <option value="">+ Add</option>
          {options
            .filter((o) => !value.includes(o))
            .map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
        </select>
      )}
    </span>
  )
}

/// The header of a column: its name, and a menu to change it.
function ColumnHeader({
  doc,
  property,
  columns,
  readonly,
}: {
  doc: Y.Doc
  property: Property
  columns: Property[]
  readonly: boolean
}) {
  const menu = useMenuAnchor()
  const [name, setName] = useState(property.name)
  useEffect(() => setName(property.name), [property.name])

  const update = (changes: Partial<Property>) =>
    writeSchema(
      doc,
      columns.map((c) => (c.id === property.id ? { ...c, ...changes } : c))
    )

  return (
    <th className="border-b border-line bg-sidebar px-0 py-0 text-left align-bottom font-normal">
      <div className="flex items-center gap-1 px-2 py-1.5">
        <input
          value={name}
          readOnly={readonly}
          aria-label={`${property.name} column name`}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => name.trim() && name !== property.name && update({ name: name.trim() })}
          onKeyDown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
          className="w-full min-w-0 bg-transparent text-[12px] font-semibold text-ink outline-none"
        />
        {!readonly && (
          <IconButton
            label="Column options"
            className="size-5"
            onClick={(e) => menu.openFrom(e.currentTarget)}
          >
            <TypeIcon size={12} />
          </IconButton>
        )}
      </div>
      <div className="px-2 pb-1.5 text-[10px] tracking-[0.04em] text-ink-3 uppercase">
        {KIND_LABELS[property.type] ?? property.type}
      </div>
      <Menu
        at={menu.at}
        onClose={menu.close}
        width={200}
        items={[
          ...KINDS.filter((k) => k !== property.type).map((k) => ({
            label: `Make it ${KIND_LABELS[k].toLowerCase()}`,
            onSelect: () => update({ type: k }),
          })),
          'separator',
          {
            label: 'Delete column',
            icon: <Trash2 size={14} />,
            danger: true,
            onSelect: () => writeSchema(doc, columns.filter((c) => c.id !== property.id)),
          },
        ]}
      />
    </th>
  )
}

/// The table view: one row per entry, one column per property.
function TableView({
  doc,
  columns,
  rows,
  readonly,
}: {
  doc: Y.Doc
  columns: Property[]
  rows: Row[]
  readonly: boolean
}) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full border-collapse text-left" data-testid="database-table">
        <thead>
          <tr>
            {columns.map((c) => (
              <ColumnHeader
                key={c.id}
                doc={doc}
                property={c}
                columns={columns}
                readonly={readonly}
              />
            ))}
            {!readonly && (
              <th className="w-8 border-b border-line bg-sidebar" />
            )}
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.id} className="group border-b border-line/70 hover:bg-hover/40">
              {columns.map((c) => (
                <td key={c.id} className="p-0 align-middle">
                  <CellEditor doc={doc} row={row} property={c} readonly={readonly} />
                </td>
              ))}
              {!readonly && (
                <td className="w-8">
                  <IconButton
                    label="Delete row"
                    className="size-6 opacity-0 group-hover:opacity-100"
                    onClick={() => removeRow(doc, row.id)}
                  >
                    <Trash2 size={12} />
                  </IconButton>
                </td>
              )}
            </tr>
          ))}
          {rows.length === 0 && (
            <tr>
              <td colSpan={Math.max(columns.length, 1)} className="px-2 py-6 text-center text-[13px] text-ink-3">
                No rows yet.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  )
}

/// The board view: the rows as cards, grouped by their first `select` column.
function BoardView({
  doc,
  columns,
  rows,
  readonly,
}: {
  doc: Y.Doc
  columns: Property[]
  rows: Row[]
  readonly: boolean
}) {
  const group = columns.find((c) => c.type === 'select') ?? columns[0]
  const [dragging, setDragging] = useState<string | null>(null)
  const [over, setOver] = useState<string | null>(null)
  if (!group) return null
  const options = group.options ?? []
  // A row can hold a value that is not one of the options (an agent wrote it),
  // so those values get a column of their own instead of disappearing.
  const extra = [
    ...new Set(
      rows
        .map((r) => cellValue(r, group.id))
        .filter((v): v is string => typeof v === 'string' && v !== '' && !options.includes(v))
    ),
  ]
  const groups = [...options, ...extra, '']
  const others = columns.filter((c) => c.id !== group.id)

  const cardsOf = (option: string) =>
    rows.filter((r) => {
      const v = cellValue(r, group.id)
      return (typeof v === 'string' ? v : '') === option
    })

  return (
    <div className="flex gap-3 overflow-x-auto pb-2" data-testid="database-board">
      {groups.map((option) => (
        <section
          key={option || 'empty'}
          className={`flex w-[248px] shrink-0 flex-col rounded-lg bg-sidebar p-2 ${
            over === option ? 'ring-1 ring-[var(--accent)]' : ''
          }`}
          onDragOver={(e) => {
            if (readonly) return
            e.preventDefault()
            setOver(option)
          }}
          onDragLeave={() => setOver((o) => (o === option ? null : o))}
          onDrop={(e) => {
            e.preventDefault()
            setOver(null)
            const id = e.dataTransfer.getData('text/plain') || dragging
            setDragging(null)
            if (!id || readonly) return
            writeCell(doc, id, group.id, option)
          }}
        >
          <h3 className="px-1 pb-2 text-[11px] font-semibold tracking-[0.04em] text-ink-3 uppercase">
            {option || 'No status'}
            <span className="ml-1.5 tabular-nums">{cardsOf(option).length}</span>
          </h3>
          <div className="flex flex-col gap-1.5">
            {cardsOf(option).map((row) => {
              const title = columns.find((c) => c.type === 'title')
              return (
                <article
                  key={row.id}
                  draggable={!readonly}
                  onDragStart={(e) => {
                    e.dataTransfer.setData('text/plain', row.id)
                    setDragging(row.id)
                  }}
                  onDragEnd={() => {
                    setDragging(null)
                    setOver(null)
                  }}
                  className={`rounded-md border border-line bg-paper px-2.5 py-2 shadow-[0_1px_2px_rgba(0,0,0,0.04)] ${
                    dragging === row.id ? 'opacity-50' : ''
                  }`}
                >
                  <p className="truncate text-[13px] font-medium text-ink">
                    {title ? cellText(cellValue(row, title.id)) || 'Untitled' : 'Untitled'}
                  </p>
                  {others.map((c) => {
                    const v = cellText(cellValue(row, c.id))
                    if (!v) return null
                    return (
                      <p key={c.id} className="truncate text-[12px] text-ink-3">
                        <span className="text-ink-3/80">{c.name}:</span> {v}
                      </p>
                    )
                  })}
                </article>
              )
            })}
            {cardsOf(option).length === 0 && (
              <p className="px-1 py-2 text-[12px] text-ink-3/70">Drop a card here</p>
            )}
          </div>
        </section>
      ))}
    </div>
  )
}

export function DatabaseView({ doc, readonly }: { doc: Y.Doc; readonly: boolean }) {
  const [version, setVersion] = useState(0)
  const [adding, setAdding] = useState(false)
  const addMenu = useMenuAnchor()
  const addInput = useRef<HTMLInputElement>(null)

  // The database lives in the document, so anything written anywhere shows up.
  useEffect(() => {
    const bump = () => setVersion((v) => v + 1)
    const rows = rowsMap(doc)
    const schema = schemaMap(doc)
    const view = viewMap(doc)
    rows.observeDeep(bump)
    schema.observe(bump)
    view.observe(bump)
    return () => {
      rows.unobserveDeep(bump)
      schema.unobserve(bump)
      view.unobserve(bump)
    }
  }, [doc])

  const columns = useMemo(() => readSchema(doc), [doc, version])
  const rows = useMemo(() => readRows(doc), [doc, version])
  const view = useMemo(() => readView(doc), [doc, version])

  const addProperty = (kind: PropertyKind) => {
    const id = newId()
    const property: Property = {
      id,
      name: KIND_LABELS[kind],
      type: kind,
      ...(kind === 'select' || kind === 'multi_select' ? { options: ['Todo'] } : {}),
    }
    writeSchema(doc, [...columns, property])
    setAdding(false)
  }

  return (
    <section className="flex flex-col gap-3" data-testid="database-view">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1 rounded-md bg-sidebar p-0.5">
          <button
            type="button"
            aria-pressed={view === 'table'}
            onClick={() => writeView(doc, 'table')}
            className={`press flex h-6 items-center gap-1.5 rounded-[5px] px-2 text-[12px] ${
              view === 'table' ? 'bg-paper text-ink shadow-[0_1px_2px_rgba(0,0,0,0.06)]' : 'text-ink-3'
            }`}
          >
            <Table2 size={12} />
            Table
          </button>
          <button
            type="button"
            aria-pressed={view === 'board'}
            onClick={() => writeView(doc, 'board')}
            className={`press flex h-6 items-center gap-1.5 rounded-[5px] px-2 text-[12px] ${
              view === 'board' ? 'bg-paper text-ink shadow-[0_1px_2px_rgba(0,0,0,0.06)]' : 'text-ink-3'
            }`}
          >
            <LayoutGrid size={12} />
            Board
          </button>
        </div>
        <div className="flex items-center gap-1.5">
          {adding && (
            <input
              ref={addInput}
              autoFocus
              placeholder="Column name"
              aria-label="New column name"
              onKeyDown={(e) => {
                if (e.key === 'Enter' && addInput.current?.value.trim()) {
                  const name = addInput.current.value.trim()
                  addInput.current.value = ''
                  setAdding(false)
                  writeSchema(doc, [
                    ...columns,
                    { id: newId(), name, type: 'text' },
                  ])
                }
                if (e.key === 'Escape') {
                  setAdding(false)
                }
              }}
              onBlur={() => setAdding(false)}
              className="h-7 w-40 rounded-md bg-paper px-2 text-[13px] text-ink shadow-[inset_0_0_0_1px_var(--line)] outline-none"
            />
          )}
          {!readonly && !adding && (
            <>
              <Button
                variant="ghost"
                className="h-7 px-2 text-[12px]"
                onClick={(e) => addMenu.openFrom(e.currentTarget)}
                data-testid="add-column"
              >
                <Plus size={13} />
                Column
              </Button>
              <Menu
                at={addMenu.at}
                onClose={addMenu.close}
                width={200}
                items={[
                  ...KINDS.map((k) => ({
                    label: KIND_LABELS[k],
                    onSelect: () => addProperty(k),
                  })),
                  'separator',
                  {
                    label: 'Name it yourself…',
                    onSelect: () => setAdding(true),
                  },
                ]}
              />
            </>
          )}
        </div>
      </div>

      {columns.length === 0 ? (
        <p className="rounded-lg border border-dashed border-line px-4 py-10 text-center text-[13px] text-ink-3">
          This database has no columns yet.
        </p>
      ) : view === 'board' ? (
        <BoardView doc={doc} columns={columns} rows={rows} readonly={readonly} />
      ) : (
        <TableView doc={doc} columns={columns} rows={rows} readonly={readonly} />
      )}

      {!readonly && (
        <div>
          <Button
            variant="subtle"
            className="h-7 px-2.5 text-[12px]"
            onClick={() => addRow(doc)}
            data-testid="add-row"
          >
            <Plus size={13} />
            New row
          </Button>
        </div>
      )}
    </section>
  )
}
