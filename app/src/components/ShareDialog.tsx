import { Copy, Globe, Link2Off, UserMinus, UserPlus } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { colorFor } from '../lib/hooks'
import { type Member, type Role, type Share, api } from '../lib/ipc'
import { currentWorkspace, useApp } from '../lib/store'
import type { Page } from '../lib/workspace'
import { thisDevice } from '../lib/platform'
import { AccountForm } from './AccountForm'
import { host } from './Sidebar'
import { Button, Dialog, IconButton, copyText, inputClass } from './ui'

export function ShareDialog({ open, onClose, page }: { open: boolean; onClose: () => void; page: Page | null }) {
  const ws = useApp(currentWorkspace)
  return (
    <Dialog open={open} onClose={onClose} title={ws?.accountId ? 'Share' : 'Share this workspace'} width={520} testId="share-dialog">
      {ws && (ws.accountId ? <RemoteShare page={page} /> : <PublishPanel />)}
    </Dialog>
  )
}

/// A local workspace must live on a server before anyone else can open it.
function PublishPanel() {
  const { accounts, refresh, fail, toast } = useApp()
  const ws = useApp(currentWorkspace)
  const loggedIn = accounts.filter((a) => a.loggedIn)
  const [accountId, setAccountId] = useState(loggedIn[0]?.id ?? '')
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    if (!accountId && loggedIn[0]) setAccountId(loggedIn[0].id)
  }, [accountId, loggedIn])

  if (!ws) return null
  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        <span className="font-medium text-ink">{ws.name}</span> lives only on {thisDevice}. Publish it to
        a server to invite collaborators, edit together live, and create public links. Everything
        stays on {thisDevice} too.
      </p>
      {loggedIn.length === 0 ? (
        <div className="rounded-lg bg-paper p-4 shadow-[inset_0_0_0_1px_var(--line)]">
          <AccountForm onDone={(a) => setAccountId(a.id)} />
        </div>
      ) : (
        <div className="flex items-center gap-2">
          <select
            className={`${inputClass} flex-1`}
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
          <Button
            variant="primary"
            disabled={busy || !accountId}
            data-testid="publish-button"
            onClick={async () => {
              setBusy(true)
              try {
                await api.publishWorkspace(ws.id, accountId)
                await refresh()
                toast('Published. Invite people from Share.')
              } catch (e) {
                fail(e)
              } finally {
                setBusy(false)
              }
            }}
          >
            {busy ? 'Publishing…' : 'Publish'}
          </Button>
        </div>
      )}
    </div>
  )
}

function RemoteShare({ page }: { page: Page | null }) {
  const ws = useApp(currentWorkspace)!
  const { accounts, fail, toast } = useApp()
  const me = accounts.find((a) => a.id === ws.accountId)
  const [members, setMembers] = useState<Member[]>([])
  const [shares, setShares] = useState<Share[]>([])
  const [username, setUsername] = useState('')
  const [role, setRole] = useState<Role>('editor')
  const isOwner = ws.role === 'owner'
  const canEdit = ws.role !== 'viewer'

  const load = useCallback(async () => {
    try {
      const [m, s] = await Promise.all([api.members(ws.id), api.shares(ws.id)])
      setMembers(m)
      setShares(s)
    } catch (e) {
      fail(e)
    }
  }, [ws.id, fail])

  useEffect(() => {
    void load()
  }, [load])

  const share = page ? shares.find((s) => s.pageId === page.id) : undefined

  return (
    <div className="flex flex-col gap-5">
      <section className="flex flex-col gap-2">
        <h3 className="text-[12px] font-semibold text-ink-2">People in {ws.name}</h3>
        {isOwner && (
          <form
            className="flex gap-2"
            onSubmit={async (e) => {
              e.preventDefault()
              if (!username.trim()) return
              try {
                await api.addMember(ws.id, username.trim().toLowerCase(), role)
                setUsername('')
                toast(`Invited @${username.trim()}`)
                await load()
              } catch (err) {
                fail(err)
              }
            }}
          >
            <input
              className={`${inputClass} flex-1`}
              placeholder="Username"
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              autoCapitalize="off"
              spellCheck={false}
              aria-label="Username to invite"
            />
            <select
              className={inputClass}
              value={role}
              onChange={(e) => setRole(e.target.value as Role)}
              aria-label="Role"
            >
              <option value="editor">Can edit</option>
              <option value="viewer">Can view</option>
            </select>
            <Button variant="primary" type="submit">
              <UserPlus size={14} />
              Invite
            </Button>
          </form>
        )}
        <div className="-mx-2 flex flex-col">
          {members.map((m) => (
            <div key={m.userId} className="flex h-10 items-center gap-2.5 rounded-md px-2 hover:bg-hover">
              <span
                className="flex size-6 items-center justify-center rounded-full text-[10px] font-semibold text-white"
                style={{ background: colorFor(m.userId) }}
              >
                {(m.displayName || m.username).slice(0, 1).toUpperCase()}
              </span>
              <span className="min-w-0 flex-1 truncate text-[13px] text-ink">
                {m.displayName || m.username}{' '}
                <span className="text-ink-3">@{m.username}</span>
                {m.userId === me?.userId && <span className="text-ink-3"> (you)</span>}
              </span>
              <span className="text-[12px] text-ink-3">
                {m.role === 'owner' ? 'Owner' : m.role === 'editor' ? 'Can edit' : 'Can view'}
              </span>
              {isOwner && m.role !== 'owner' && (
                <IconButton
                  label={`Remove @${m.username}`}
                  className="hover:text-danger"
                  onClick={async () => {
                    try {
                      await api.removeMember(ws.id, m.userId)
                      await load()
                    } catch (e) {
                      fail(e)
                    }
                  }}
                >
                  <UserMinus size={14} />
                </IconButton>
              )}
            </div>
          ))}
        </div>
        {!isOwner && (
          <p className="text-[12px] text-ink-3">Only the owner can invite or remove people.</p>
        )}
      </section>

      <section className="flex flex-col gap-2 border-t border-line pt-4">
        <h3 className="flex items-center gap-1.5 text-[12px] font-semibold text-ink-2">
          <Globe size={13} />
          Public link{page ? ` to “${page.title || 'Untitled'}”` : ''}
        </h3>
        {!page ? (
          <p className="text-[12px] text-ink-3">Open a page to create a read-only link to it.</p>
        ) : share ? (
          <>
            <div className="flex items-center gap-2">
              <code className="selectable min-w-0 flex-1 truncate rounded-md bg-paper px-2.5 py-1.5 font-mono text-[12px] shadow-[inset_0_0_0_1px_var(--line)]" data-testid="share-url">
                {share.url}
              </code>
              <IconButton
                label="Copy link"
                onClick={async () => {
                  await copyText(share.url)
                  toast('Link copied')
                }}
              >
                <Copy size={14} />
              </IconButton>
              {canEdit && (
                <IconButton
                  label="Revoke link"
                  className="hover:text-danger"
                  onClick={async () => {
                    try {
                      await api.deleteShare(ws.id, share.token)
                      await load()
                    } catch (e) {
                      fail(e)
                    }
                  }}
                >
                  <Link2Off size={14} />
                </IconButton>
              )}
            </div>
            <p className="text-[12px] text-ink-3">
              Anyone with the link can read this page and the pages inside it. They can't edit.
            </p>
          </>
        ) : canEdit ? (
          <Button
            variant="subtle"
            className="self-start"
            data-testid="create-share"
            onClick={async () => {
              try {
                await api.createShare(ws.id, page.id)
                await load()
              } catch (e) {
                fail(e)
              }
            }}
          >
            Create read-only link
          </Button>
        ) : (
          <p className="text-[12px] text-ink-3">Viewers can't create links.</p>
        )}
      </section>
    </div>
  )
}
