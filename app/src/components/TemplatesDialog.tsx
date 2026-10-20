import { FileText } from 'lucide-react'
import type { DocHandle } from '../lib/docs'
import { api } from '../lib/ipc'
import { useApp } from '../lib/store'
import { BUILT_IN, type Template, fill, templatesPage, userTemplates } from '../lib/templates'
import { type Page, setPageIcon } from '../lib/workspace'
import { Dialog } from './ui'

/// "New from template": built-in templates and the pages under "Templates".
export function TemplatesDialog({
  open,
  onClose,
  workspaceId,
  wsHandle,
  live,
}: {
  open: boolean
  onClose: () => void
  workspaceId: string | null
  wsHandle: DocHandle | null
  live: Page[]
}) {
  const { openPage, fail } = useApp()
  const own = userTemplates(live)
  const home = templatesPage(live)

  const create = async (t: Template) => {
    if (!workspaceId || !wsHandle) return
    try {
      const title = fill(t.title)
      if (t.id === 'daily') {
        const existing = live.find((p) => p.title === title)
        if (existing) {
          openPage(existing.id)
          onClose()
          return
        }
      }
      const markdown = t.pageId ? await api.exportMarkdown(workspaceId, t.pageId) : (t.markdown ?? '')
      const id = await api.importMarkdown(workspaceId, '', title, fill(markdown))
      if (t.icon) setPageIcon(wsHandle.doc, id, t.icon)
      openPage(id)
      onClose()
    } catch (e) {
      fail(e)
    }
  }

  const card = (t: Template) => (
    <button
      key={t.id}
      type="button"
      onClick={() => void create(t)}
      className="press flex items-start gap-3 rounded-lg p-3 text-left shadow-[inset_0_0_0_1px_var(--line)] hover:bg-hover"
    >
      <span className="flex size-7 shrink-0 items-center justify-center text-[20px] leading-none">
        {t.icon || <FileText size={16} className="text-ink-3" />}
      </span>
      <span className="min-w-0">
        <span className="block truncate text-[13px] font-medium text-ink">{t.name}</span>
        {t.description && <span className="mt-0.5 block text-[12px] leading-snug text-ink-3">{t.description}</span>}
      </span>
    </button>
  )

  return (
    <Dialog open={open} onClose={onClose} title="New from template" width={560} testId="templates-dialog">
      <div className="grid grid-cols-2 gap-2">{BUILT_IN.map(card)}</div>
      <h3 className="mt-5 mb-2 text-[12px] font-medium text-ink-2">Your templates</h3>
      {own.length > 0 ? (
        <div className="grid grid-cols-2 gap-2">{own.map(card)}</div>
      ) : (
        <p className="text-[12px] leading-relaxed text-ink-3">
          {home
            ? 'Add pages under “Templates” and they show up here.'
            : 'Make a top-level page called “Templates”; every page you put under it becomes a template.'}{' '}
          Use <code className="font-mono">{'{{date}}'}</code> and <code className="font-mono">{'{{weekday}}'}</code> for
          today’s values.
        </p>
      )}
    </Dialog>
  )
}
