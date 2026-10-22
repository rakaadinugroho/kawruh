/// The inbox: mentions and replies, newest first.
import { AtSign, MessageSquareReply } from 'lucide-react'
import { useEffect } from 'react'
import { type Notification, useInbox } from '../lib/inbox'
import { Button, Dialog } from './ui'

const ago = (ms: number) => {
  const s = Math.max(1, Math.round((Date.now() - ms) / 1000))
  if (s < 60) return 'just now'
  const m = Math.round(s / 60)
  if (m < 60) return `${m}m ago`
  const h = Math.round(m / 60)
  if (h < 24) return `${h}h ago`
  return `${Math.round(h / 24)}d ago`
}

export function InboxDialog({
  open,
  onClose,
  accountId,
  onOpen,
}: {
  open: boolean
  onClose: () => void
  accountId: string | null
  onOpen: (n: Notification) => void
}) {
  const inbox = useInbox((s) => (accountId ? (s.byAccount[accountId] ?? { items: [], unread: 0 }) : { items: [], unread: 0 }))
  const markRead = useInbox((s) => s.markRead)

  useEffect(() => {
    if (!open || !accountId) return
    const unread = inbox.items.filter((n) => n.readAt === null).map((n) => n.id)
    if (unread.length) void markRead(accountId, unread)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, accountId])

  const openItem = (n: Notification) => {
    onOpen(n)
    onClose()
  }

  return (
    <Dialog open={open} onClose={onClose} title="Inbox" width={520} testId="inbox-dialog">
      {inbox.items.length === 0 ? (
        <p className="py-6 text-center text-[13px] text-ink-3">
          Nothing here yet. Mentions and replies land in this box.
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {inbox.items.map((n) => (
            <li key={n.id}>
              <button
                type="button"
                onClick={() => openItem(n)}
                data-testid="inbox-item"
                className="press flex w-full items-start gap-2.5 rounded-md px-2 py-2 text-left hover:bg-hover"
              >
                <span className="mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full bg-accent-soft text-accent">
                  {n.kind === 'mention' ? <AtSign size={13} /> : <MessageSquareReply size={13} />}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="flex items-baseline gap-1.5 text-[12px]">
                    <span className="font-semibold text-ink">{n.authorName || 'Someone'}</span>
                    <span className="text-ink-3">{ago(n.createdAt)}</span>
                    {n.readAt === null && <span className="size-1.5 rounded-full bg-accent" />}
                  </span>
                  <span className="mt-0.5 line-clamp-2 block text-[13px] text-ink-2">{n.body}</span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
      {inbox.items.length > 0 && (
        <div className="mt-3 flex justify-end">
          <Button
            variant="ghost"
            className="h-7 px-2 text-[12px]"
            onClick={() => accountId && void markRead(accountId, [])}
          >
            Mark all as read
          </Button>
        </div>
      )}
    </Dialog>
  )
}
