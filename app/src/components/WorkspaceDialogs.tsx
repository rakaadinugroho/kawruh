import { Check, Laptop, Server } from 'lucide-react'
import { useEffect, useState } from 'react'
import { type RemoteWorkspace, api } from '../lib/ipc'
import { useApp } from '../lib/store'
import { isMac } from '../lib/platform'
import { AccountForm } from './AccountForm'
import { host } from './Sidebar'
import { Button, Dialog, Field, inputClass } from './ui'

export function NewWorkspaceDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { accounts, refresh, selectWorkspace, fail } = useApp()
  const loggedIn = accounts.filter((a) => a.loggedIn)
  const [name, setName] = useState('')
  const [where, setWhere] = useState<string>('local')
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (open) {
      setName('')
      setWhere('local')
    }
  }, [open])

  const options = [
    { id: 'local', label: isMac ? 'This Mac only' : 'This computer only', detail: 'Private. Publish to a server any time.', icon: <Laptop size={15} /> },
    ...loggedIn.map((a) => ({
      id: a.id,
      label: host(a.serverUrl),
      detail: `Shared-ready, as @${a.username}`,
      icon: <Server size={15} />,
    })),
  ]

  return (
    <Dialog open={open} onClose={onClose} title="New workspace" width={440} testId="new-workspace-dialog">
      <form
        className="flex flex-col gap-4"
        onSubmit={async (e) => {
          e.preventDefault()
          setBusy(true)
          try {
            const ws = await api.createWorkspace(name, where === 'local' ? null : where)
            await refresh()
            selectWorkspace(ws.id)
            onClose()
          } catch (err) {
            fail(err)
          } finally {
            setBusy(false)
          }
        }}
      >
        <Field label="Name">
          <input
            autoFocus
            className={inputClass}
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Research notes"
            name="workspaceName"
          />
        </Field>
        <div className="flex flex-col gap-1.5">
          <span className="text-[12px] font-medium text-ink-2">Where it lives</span>
          {options.map((o) => (
            <button
              key={o.id}
              type="button"
              onClick={() => setWhere(o.id)}
              className={`press flex items-center gap-3 rounded-md px-3 py-2 text-left ${
                where === o.id ? 'bg-accent-soft shadow-[inset_0_0_0_1.5px_var(--accent)]' : 'bg-paper shadow-[inset_0_0_0_1px_var(--line)] hover:bg-hover'
              }`}
            >
              <span className="text-ink-2">{o.icon}</span>
              <span className="flex-1">
                <span className="block text-[13px] font-medium text-ink">{o.label}</span>
                <span className="block text-[12px] text-ink-3">{o.detail}</span>
              </span>
              {where === o.id && <Check size={14} className="text-accent" />}
            </button>
          ))}
        </div>
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" disabled={busy}>
            {busy ? 'Creating…' : 'Create workspace'}
          </Button>
        </div>
      </form>
    </Dialog>
  )
}

export function JoinDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { accounts, refresh, selectWorkspace, fail } = useApp()
  const loggedIn = accounts.filter((a) => a.loggedIn)
  const [accountId, setAccountId] = useState(loggedIn[0]?.id ?? '')
  const [list, setList] = useState<RemoteWorkspace[] | null>(null)

  useEffect(() => {
    if (!accountId && loggedIn[0]) setAccountId(loggedIn[0].id)
  }, [accountId, loggedIn])

  useEffect(() => {
    if (!open || !accountId) return
    setList(null)
    api.remoteWorkspaces(accountId).then(setList).catch((e) => {
      fail(e)
      setList([])
    })
  }, [open, accountId, fail])

  return (
    <Dialog open={open} onClose={onClose} title="Add a workspace from a server" width={480} testId="join-dialog">
      {loggedIn.length === 0 ? (
        <AccountForm onDone={(a) => setAccountId(a.id)} />
      ) : (
        <div className="flex flex-col gap-3">
          {loggedIn.length > 1 && (
            <select
              className={inputClass}
              value={accountId}
              onChange={(e) => setAccountId(e.target.value)}
              aria-label="Server account"
            >
              {loggedIn.map((a) => (
                <option key={a.id} value={a.id}>
                  @{a.username} on {host(a.serverUrl)}
                </option>
              ))}
            </select>
          )}
          {list === null ? (
            <p className="py-4 text-center text-[13px] text-ink-3">Loading…</p>
          ) : list.length === 0 ? (
            <p className="py-4 text-center text-[13px] text-ink-3">
              No workspaces yet. Ask someone to invite you, or create one.
            </p>
          ) : (
            <div className="-mx-2 flex flex-col">
              {list.map((w) => (
                <div key={w.id} className="flex h-10 items-center gap-3 rounded-md px-2 hover:bg-hover">
                  <span className="min-w-0 flex-1 truncate text-[13px] text-ink">{w.name}</span>
                  <span className="text-[12px] text-ink-3">
                    {w.role === 'owner' ? 'Owner' : w.role === 'editor' ? 'Can edit' : 'Can view'}
                  </span>
                  {w.attached ? (
                    <Button
                      variant="ghost"
                      className="h-7"
                      onClick={() => {
                        selectWorkspace(w.id)
                        onClose()
                      }}
                    >
                      Open
                    </Button>
                  ) : (
                    <Button
                      variant="subtle"
                      className="h-7"
                      onClick={async () => {
                        try {
                          await api.attachWorkspace(accountId, w.id)
                          await refresh()
                          selectWorkspace(w.id)
                          onClose()
                        } catch (e) {
                          fail(e)
                        }
                      }}
                    >
                      Add
                    </Button>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </Dialog>
  )
}
