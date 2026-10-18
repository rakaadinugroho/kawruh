import { FileText, RotateCcw, Trash2 } from 'lucide-react'
import type * as Y from 'yjs'
import { type Page, purgePage, restorePage } from '../lib/workspace'
import { Dialog, IconButton } from './ui'

function ago(ms: number) {
  const s = Math.max(1, Math.round((Date.now() - ms) / 1000))
  if (s < 60) return 'just now'
  const m = Math.round(s / 60)
  if (m < 60) return `${m} min ago`
  const h = Math.round(m / 60)
  if (h < 24) return `${h} h ago`
  return new Date(ms).toLocaleDateString()
}

export function TrashDialog({ open, onClose, doc, trash, readonly, onOpen }: {
  open: boolean
  onClose: () => void
  doc: Y.Doc | null
  trash: Page[]
  readonly: boolean
  onOpen: (id: string) => void
}) {
  return (
    <Dialog open={open} onClose={onClose} title="Trash" width={480} testId="trash-dialog">
      {trash.length === 0 ? (
        <p className="py-6 text-center text-[13px] text-ink-3">Trash is empty.</p>
      ) : (
        <div className="-mx-2 flex max-h-[360px] flex-col overflow-y-auto">
          {trash.map((p) => (
            <div key={p.id} className="group flex h-9 items-center gap-2 rounded-md px-2 hover:bg-hover">
              <button
                type="button"
                className="flex min-w-0 flex-1 items-center gap-2 text-left text-[13px]"
                onClick={() => {
                  onOpen(p.id)
                  onClose()
                }}
              >
                <span className="flex w-5 justify-center text-[14px]">
                  {p.icon || <FileText size={14} className="text-ink-3" />}
                </span>
                <span className={`truncate ${p.title ? 'text-ink' : 'text-ink-3'}`}>
                  {p.title || 'Untitled'}
                </span>
                <span className="shrink-0 text-[12px] text-ink-3">{ago(p.deleted)}</span>
              </button>
              {!readonly && doc && (
                <>
                  <IconButton label="Restore" onClick={() => restorePage(doc, p.id)}>
                    <RotateCcw size={14} />
                  </IconButton>
                  <IconButton
                    label="Delete forever"
                    className="hover:text-danger"
                    onClick={() => purgePage(doc, p.id)}
                  >
                    <Trash2 size={14} />
                  </IconButton>
                </>
              )}
            </div>
          ))}
        </div>
      )}
    </Dialog>
  )
}
