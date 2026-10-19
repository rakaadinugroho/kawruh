import { Copy, KeyRound, LogOut, Plus, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import type { DocFont, DocSize } from '../lib/appearance'
import { type ApiToken, type AppInfo, api } from '../lib/ipc'
import { useApp } from '../lib/store'
import { thisDevice } from '../lib/platform'
import { AccountForm } from './AccountForm'
import { host } from './Sidebar'
import { Button, Dialog, Field, IconButton, copyText, inputClass } from './ui'

type Tab = 'accounts' | 'appearance' | 'tokens' | 'about'

export function SettingsDialog({ open, tab: initialTab, onClose }: {
  open: boolean
  tab?: Tab
  onClose: () => void
}) {
  const [tab, setTab] = useState<Tab>(initialTab ?? 'accounts')
  useEffect(() => {
    if (open) setTab(initialTab ?? 'accounts')
  }, [open, initialTab])

  return (
    <Dialog open={open} onClose={onClose} title="Settings" width={560} testId="settings-dialog">
      <div className="mb-4 flex gap-1 rounded-md bg-hover p-0.5" role="tablist">
        {(
          [
            ['accounts', 'Accounts'],
            ['appearance', 'Appearance'],
            ['tokens', 'API tokens'],
            ['about', 'About'],
          ] as const
        ).map(([key, label]) => (
          <button
            key={key}
            type="button"
            role="tab"
            aria-selected={tab === key}
            onClick={() => setTab(key)}
            className={`press h-7 flex-1 rounded-[5px] text-[12px] font-medium ${
              tab === key ? 'bg-raised text-ink shadow-[0_1px_2px_oklch(0_0_0/0.08)]' : 'text-ink-2'
            }`}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === 'accounts' && <AccountsTab />}
      {tab === 'appearance' && <AppearanceTab />}
      {tab === 'tokens' && <TokensTab />}
      {tab === 'about' && <AboutTab />}
    </Dialog>
  )
}

const FONTS: { key: DocFont; label: string; detail: string }[] = [
  { key: 'serif', label: 'Serif', detail: 'Literata' },
  { key: 'sans', label: 'Sans', detail: 'System' },
  { key: 'mono', label: 'Mono', detail: 'Monospace' },
]

const SIZES: { key: DocSize; label: string }[] = [
  { key: 'small', label: 'Small' },
  { key: 'default', label: 'Default' },
  { key: 'large', label: 'Large' },
]

function AppearanceTab() {
  const appearance = useApp((s) => s.appearance)
  const setAppearance = useApp((s) => s.setAppearance)
  return (
    <div className="flex flex-col gap-5">
      <section>
        <h3 className="mb-2 text-[12px] font-medium text-ink-2">Page font</h3>
        <div className="grid grid-cols-3 gap-2" role="radiogroup" aria-label="Page font">
          {FONTS.map((f) => {
            const selected = appearance.font === f.key
            return (
              <button
                key={f.key}
                type="button"
                role="radio"
                aria-checked={selected}
                onClick={() => setAppearance({ font: f.key })}
                className={`press flex flex-col items-start gap-1.5 rounded-lg p-3 text-left ${
                  selected
                    ? 'bg-accent-soft shadow-[inset_0_0_0_1.5px_var(--accent)]'
                    : 'shadow-[inset_0_0_0_1px_var(--line)] hover:bg-hover'
                }`}
              >
                <span
                  className="text-[30px] leading-none text-ink"
                  style={{ fontFamily: `var(--font-${f.key})` }}
                  aria-hidden
                >
                  Ag
                </span>
                <span className="text-[13px] font-medium text-ink">{f.label}</span>
                <span className="text-[11px] text-ink-3">{f.detail}</span>
              </button>
            )
          })}
        </div>
      </section>
      <section>
        <h3 className="mb-2 text-[12px] font-medium text-ink-2">Text size</h3>
        <div className="flex gap-1 rounded-md bg-hover p-0.5" role="radiogroup" aria-label="Text size">
          {SIZES.map((z) => (
            <button
              key={z.key}
              type="button"
              role="radio"
              aria-checked={appearance.size === z.key}
              onClick={() => setAppearance({ size: z.key })}
              className={`press h-7 flex-1 rounded-[5px] text-[12px] font-medium ${
                appearance.size === z.key
                  ? 'bg-raised text-ink shadow-[0_1px_2px_oklch(0_0_0/0.08)]'
                  : 'text-ink-2'
              }`}
            >
              {z.label}
            </button>
          ))}
        </div>
      </section>
      <p className="text-[12px] leading-relaxed text-ink-3">
        Used for pages and exported PDFs on {thisDevice}. Collaborators keep their own choice.
      </p>
    </div>
  )
}

function AccountsTab() {
  const { accounts, refresh, fail } = useApp()
  const [adding, setAdding] = useState<null | { serverUrl?: string; username?: string }>(
    accounts.length === 0 ? {} : null
  )

  return (
    <div className="flex flex-col gap-4">
      {accounts.length > 0 && (
        <div className="flex flex-col gap-1">
          {accounts.map((a) => (
            <div key={a.id} className="flex items-center gap-3 rounded-md px-2 py-2 hover:bg-hover">
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px] font-medium text-ink">
                  {a.displayName || a.username}{' '}
                  <span className="font-normal text-ink-3">@{a.username}</span>
                </div>
                <div className="truncate text-[12px] text-ink-3">
                  {host(a.serverUrl)} · {a.loggedIn ? 'Logged in' : 'Logged out'}
                </div>
              </div>
              {a.loggedIn ? (
                <Button
                  variant="ghost"
                  className="h-7"
                  onClick={async () => {
                    try {
                      await api.logout(a.id)
                      await refresh()
                    } catch (e) {
                      fail(e)
                    }
                  }}
                >
                  <LogOut size={13} />
                  Log out
                </Button>
              ) : (
                <Button
                  variant="subtle"
                  className="h-7"
                  onClick={() => setAdding({ serverUrl: a.serverUrl, username: a.username })}
                >
                  Log in
                </Button>
              )}
            </div>
          ))}
        </div>
      )}
      {adding ? (
        <div className="rounded-lg bg-paper p-4 shadow-[inset_0_0_0_1px_var(--line)]">
          <AccountForm
            initial={adding}
            onDone={() => setAdding(null)}
            onCancel={accounts.length ? () => setAdding(null) : undefined}
          />
        </div>
      ) : (
        <Button variant="subtle" className="self-start" onClick={() => setAdding({})}>
          <Plus size={14} />
          Connect a server
        </Button>
      )}
    </div>
  )
}

function TokensTab() {
  const { accounts, fail } = useApp()
  const loggedIn = accounts.filter((a) => a.loggedIn)
  const [accountId, setAccountId] = useState(loggedIn[0]?.id ?? '')
  const [tokens, setTokens] = useState<ApiToken[]>([])
  const [name, setName] = useState('Claude Code')
  const [created, setCreated] = useState<ApiToken | null>(null)
  const account = loggedIn.find((a) => a.id === accountId)

  useEffect(() => {
    if (!accountId) return
    api.tokens(accountId).then(setTokens).catch(fail)
  }, [accountId, fail])

  if (loggedIn.length === 0) {
    return (
      <p className="text-[13px] text-ink-2">
        API tokens let AI agents use your pages through the server's MCP endpoint. Log in to a
        server under Accounts first.
      </p>
    )
  }

  const mcpCommand =
    created?.token && account
      ? `claude mcp add --transport http kawruh ${account.serverUrl}/mcp --header "Authorization: Bearer ${created.token}"`
      : ''

  return (
    <div className="flex flex-col gap-4">
      <p className="text-[13px] leading-relaxed text-ink-2">
        Tokens let AI agents read and write your pages through MCP. Their edits show up live, like a
        collaborator's.
      </p>
      {loggedIn.length > 1 && (
        <Field label="Server account">
          <select
            className={inputClass}
            value={accountId}
            onChange={(e) => {
              setAccountId(e.target.value)
              setCreated(null)
            }}
          >
            {loggedIn.map((a) => (
              <option key={a.id} value={a.id}>
                @{a.username} on {host(a.serverUrl)}
              </option>
            ))}
          </select>
        </Field>
      )}
      <form
        className="flex items-end gap-2"
        onSubmit={async (e) => {
          e.preventDefault()
          try {
            const t = await api.createToken(accountId, name.trim() || 'Token')
            setCreated(t)
            setTokens(await api.tokens(accountId))
          } catch (err) {
            fail(err)
          }
        }}
      >
        <div className="flex-1">
          <Field label="New token name">
            <input className={inputClass} value={name} onChange={(e) => setName(e.target.value)} />
          </Field>
        </div>
        <Button variant="primary" type="submit">
          <KeyRound size={14} />
          Create token
        </Button>
      </form>
      {created?.token && (
        <div className="flex flex-col gap-2 rounded-lg bg-accent-soft p-3" data-testid="new-token">
          <p className="text-[12px] font-medium text-ink">Copy it now. It won't be shown again.</p>
          <div className="flex items-center gap-2">
            <code className="selectable min-w-0 flex-1 truncate rounded-md bg-paper px-2 py-1.5 font-mono text-[12px]">
              {created.token}
            </code>
            <IconButton label="Copy token" onClick={() => copyText(created.token!)}>
              <Copy size={14} />
            </IconButton>
          </div>
          <p className="mt-1 text-[12px] text-ink-2">Add it to Claude Code:</p>
          <div className="flex items-start gap-2">
            <code className="selectable min-w-0 flex-1 rounded-md bg-paper px-2 py-1.5 font-mono text-[11px] leading-relaxed break-all">
              {mcpCommand}
            </code>
            <IconButton label="Copy command" onClick={() => copyText(mcpCommand)}>
              <Copy size={14} />
            </IconButton>
          </div>
        </div>
      )}
      {tokens.length > 0 && (
        <div className="flex flex-col">
          {tokens.map((t) => (
            <div key={t.id} className="flex h-9 items-center gap-2 rounded-md px-2 hover:bg-hover">
              <KeyRound size={13} className="text-ink-3" />
              <span className="flex-1 truncate text-[13px] text-ink">{t.name}</span>
              <span className="text-[12px] text-ink-3">
                {t.lastUsedAt ? `used ${new Date(t.lastUsedAt).toLocaleDateString()}` : 'never used'}
              </span>
              <IconButton
                label="Revoke token"
                className="hover:text-danger"
                onClick={async () => {
                  try {
                    await api.deleteToken(accountId, t.id)
                    setTokens(await api.tokens(accountId))
                  } catch (e) {
                    fail(e)
                  }
                }}
              >
                <Trash2 size={14} />
              </IconButton>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

function AboutTab() {
  const [info, setInfo] = useState<AppInfo | null>(null)
  useEffect(() => {
    api.appInfo().then(setInfo).catch(() => {})
  }, [])
  return (
    <div className="flex flex-col gap-3 text-[13px] text-ink-2">
      <p>
        <span className="font-serif text-[17px] text-ink">Kawruh</span>{' '}
        <span className="tabular-nums">{info?.version}</span>
      </p>
      <p className="leading-relaxed">
        Kawruh is Javanese for knowledge. Pages are markdown underneath, stored on {thisDevice} and
        synced to your server when a workspace is shared.
      </p>
      {info && (
        <p className="text-[12px] text-ink-3">
          Data folder: <code className="selectable font-mono">{info.dataDir}</code>
        </p>
      )}
    </div>
  )
}
