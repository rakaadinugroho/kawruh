import { useEffect, useRef, useState } from 'react'
import { type HistoryEntry, api } from '../lib/ipc'
import { useApp } from '../lib/store'
import { thisDevice } from '../lib/platform'
import { Button, Dialog } from './ui'

const relative = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' })
const absolute = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' })

function ago(ts: number): string {
  const minutes = Math.round((ts - Date.now()) / 60_000)
  if (Math.abs(minutes) < 60) return relative.format(minutes, 'minute')
  const hours = Math.round(minutes / 60)
  if (Math.abs(hours) < 24) return relative.format(hours, 'hour')
  return absolute.format(ts)
}

/// Earlier versions of a page, saved on this device; restoring one replaces the
/// page (and syncs like any edit).
export function HistoryDialog({
  open,
  onClose,
  workspaceId,
  pageId,
  readonly,
}: {
  open: boolean
  onClose: () => void
  workspaceId: string
  pageId: string
  readonly: boolean
}) {
  const { toast, fail } = useApp()
  const [entries, setEntries] = useState<HistoryEntry[] | null>(null)
  const [selected, setSelected] = useState<HistoryEntry | null>(null)
  const preview = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    setEntries(null)
    setSelected(null)
    api
      .pageHistory(pageId)
      .then((list) => {
        setEntries(list)
        setSelected(list[0] ?? null)
      })
      .catch((e) => fail(e))
  }, [open, pageId, fail])

  useEffect(() => {
    const box = preview.current
    if (!box || !selected) return
    let stale = false
    box.replaceChildren()
    api
      .pageHistoryGet(pageId, selected.id)
      .then(async (markdown) => {
        const { renderMarkdown } = await import('../editor/print')
        const body = await renderMarkdown(markdown)
        if (!stale) box.replaceChildren(body)
      })
      .catch((e) => fail(e))
    return () => {
      stale = true
    }
  }, [selected, pageId, fail])

  const restore = async () => {
    if (!selected) return
    try {
      await api.pageHistoryRestore(workspaceId, pageId, selected.id)
      toast(`Restored the version from ${ago(selected.ts)}`)
      onClose()
    } catch (e) {
      fail(e)
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title="Page history" width={760} testId="history-dialog">
      {entries && entries.length === 0 ? (
        <p className="py-6 text-center text-[13px] text-ink-3">
          No earlier versions yet. Kawruh saves one when you open a page and every ten minutes while
          it changes.
        </p>
      ) : (
        <div className="flex h-[min(56vh,480px)] gap-4">
          <div className="-ml-1 flex w-52 shrink-0 flex-col gap-0.5 overflow-y-auto pr-1" role="listbox" aria-label="Versions">
            {(entries ?? []).map((e) => (
              <button
                key={e.id}
                type="button"
                role="option"
                aria-selected={selected?.id === e.id}
                onClick={() => setSelected(e)}
                className={`press flex flex-col items-start gap-0.5 rounded-md px-2.5 py-2 text-left ${
                  selected?.id === e.id ? 'bg-accent-soft' : 'hover:bg-hover'
                }`}
              >
                <span className="text-[13px] font-medium text-ink" title={absolute.format(e.ts)}>
                  {ago(e.ts)}
                </span>
                <span className="line-clamp-2 text-[11px] leading-snug text-ink-3">{e.preview || 'Empty'}</span>
              </button>
            ))}
          </div>
          <div
            ref={preview}
            className="kw-preview min-w-0 flex-1 overflow-y-auto rounded-lg px-5 py-4 shadow-[inset_0_0_0_1px_var(--line)]"
          />
        </div>
      )}
      <div className="mt-4 flex items-center justify-between gap-3">
        <p className="text-[12px] text-ink-3">Versions are kept on {thisDevice}, newest fifty per page.</p>
        {!readonly && (
          <Button variant="primary" disabled={!selected} onClick={() => void restore()}>
            Restore this version
          </Button>
        )}
      </div>
    </Dialog>
  )
}
