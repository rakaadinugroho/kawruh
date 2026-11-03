import { Copy, KeyRound, LogOut, Plus, Trash2, UserPlus } from 'lucide-react'
import { useEffect, useState } from 'react'
import type { DocFont, DocSize } from '../lib/appearance'
import { useAssistAgent } from '../lib/assist'
import {
  type ApiToken,
  type AppInfo,
  type McpLocalState,
  type Registration,
  api,
  errorMessage,
} from '../lib/ipc'
import { useApp } from '../lib/store'
import { thisDevice } from '../lib/platform'
import { AccountForm } from './AccountForm'
import { host } from './Sidebar'
import { Button, Dialog, Field, IconButton, Kbd, copyText, inputClass } from './ui'

type Tab = 'accounts' | 'assist' | 'agents' | 'appearance' | 'tokens' | 'about'

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
            ['assist', 'Assist'],
            ['agents', 'Agents'],
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
      {tab === 'assist' && <AssistTab />}
      {tab === 'agents' && <AgentsTab />}
      {tab === 'appearance' && <AppearanceTab />}
      {tab === 'tokens' && <TokensTab />}
      {tab === 'about' && <AboutTab />}
    </Dialog>
  )
}

/// Which agent answers Assist, and the short manual for it.
function AssistTab() {
  const { agents, chosen, choose } = useAssistAgent()
  return (
    <div className="flex flex-col gap-5">
      <section>
        <h3 className="mb-1 text-[12px] font-medium text-ink-2">Who helps you write</h3>
        <p className="mb-2.5 text-[12.5px] text-ink-2">
          Assist runs the AI agents already installed on this computer; Kawruh
          itself keeps no provider accounts or API keys. The one picked here
          answers by default.
        </p>
        <div className="grid grid-cols-1 gap-2" role="radiogroup" aria-label="Assist agent">
          {agents.map((agent) => {
            const selected = chosen?.id === agent.id
            return (
              <button
                key={agent.id}
                type="button"
                role="radio"
                aria-checked={selected}
                disabled={!agent.available}
                onClick={() => choose(agent.id)}
                className={`press flex flex-col items-start gap-0.5 rounded-lg p-3 text-left ${
                  selected
                    ? 'bg-accent-soft shadow-[inset_0_0_0_1.5px_var(--accent)]'
                    : 'shadow-[inset_0_0_0_1px_var(--line)] hover:bg-hover'
                } ${agent.available ? '' : 'opacity-55'}`}
              >
                <span className="text-[13px] font-medium text-ink">{agent.label}</span>
                <span className="text-[11px] text-ink-3 font-mono break-all">
                  {agent.available ? agent.path : `Not found — the ${agent.id} CLI isn't installed`}
                </span>
              </button>
            )
          })}
          {agents.length === 0 && (
            <p className="text-[12.5px] text-ink-3">Looking for agents…</p>
          )}
        </div>
      </section>
      <section>
        <h3 className="mb-1 text-[12px] font-medium text-ink-2">How it works</h3>
        <p className="text-[12.5px] leading-[1.6] text-ink-2">
          Select text and press <Kbd>⌘J</Kbd> to have the agent rewrite it —
          every change shows up and you apply it or not. In an empty block,
          type <code className="font-mono">'/'</code> and pick “Draft with AI”
          to have one drafted where the cursor is. Everything arrives as plain
          markdown, so nothing mysterious lands in the page.
        </p>
      </section>
    </div>
  )
}

/// A small switch matching the app's buttons (no shared Toggle exists).
function Switch({ on, label, onChange }: { on: boolean; label: string; onChange: () => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      data-testid="mcp-toggle"
      onClick={onChange}
      className={`press h-5 w-9 shrink-0 rounded-full p-0.5 ${on ? 'bg-accent' : 'bg-line'}`}
    >
      <span
        className={`block size-4 rounded-full bg-white shadow transition-transform ${
          on ? 'translate-x-4' : ''
        }`}
      />
    </button>
  )
}

/// The local MCP endpoint: agents on this computer (claude, opencode, agy)
/// read and edit the workspaces that only live on this Mac, authorized with
/// the token below. Server-backed workspaces stay out of it.
function AgentsTab() {
  const [state, setState] = useState<McpLocalState | null>(null)
  const [regs, setRegs] = useState<Registration[]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [revealed, setRevealed] = useState(false)
  const [error, setError] = useState('')

  useEffect(() => {
    api.mcpLocalState().then(setState).catch((e) => setError(errorMessage(e)))
    api.mcpStatus().then(setRegs).catch(() => {})
  }, [])

  const toggle = async () => {
    setError('')
    try {
      setState(await api.mcpLocalSet(!state?.enabled))
    } catch (e) {
      setError(errorMessage(e))
    }
  }

  const register = async (id: string, connect: boolean) => {
    setBusy(id)
    setError('')
    try {
      setRegs(await api.mcpRegister(id, connect))
    } catch (e) {
      setError(errorMessage(e))
    } finally {
      setBusy(null)
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <section>
        <div className="flex items-start justify-between gap-3">
          <div>
            <h3 className="mb-1 text-[12px] font-medium text-ink-2">Local MCP endpoint</h3>
            <p className="mb-2.5 max-w-[36ch] text-[12.5px] text-ink-2">
              Agents on this computer can read and edit the workspaces that
              never leave this Mac — pages update live while you watch. Workspaces
              that sync to a server stay out of it.
            </p>
          </div>
          <Switch on={!!state?.enabled} label="Local MCP endpoint" onChange={toggle} />
        </div>
        <div
          className={`flex flex-col gap-1.5 rounded-lg p-3 font-mono text-[11.5px] ${
            state?.enabled ? 'bg-hover' : 'bg-hover opacity-55'
          }`}
        >
          <div className="flex items-center justify-between gap-2">
            <span className="text-ink-2">URL</span>
            <span className="flex items-center gap-1 text-ink" data-testid="mcp-url">
              {state?.url ?? '…'}
              {state?.url && (
                <IconButton label="Copy URL" className="size-5" onClick={() => copyText(state.url)}>
                  <Copy size={12} />
                </IconButton>
              )}
            </span>
          </div>
          <div className="flex items-center justify-between gap-2">
            <span className="text-ink-2">Token</span>
            <span className="flex items-center gap-1 text-ink" data-testid="mcp-token">
              {revealed && state ? state.token : '••••••••••••'}
              <button
                type="button"
                className="press px-1 text-[11px] font-sans text-accent"
                onClick={() => setRevealed(!revealed)}
              >
                {revealed ? 'Hide' : 'Show'}
              </button>
              {state?.token && (
                <IconButton label="Copy token" className="size-5" onClick={() => copyText(state.token)}>
                  <Copy size={12} />
                </IconButton>
              )}
            </span>
          </div>
          <span className="text-[10.5px] text-ink-3 font-sans">
            {state?.enabled
              ? state.running
                ? `Running on 127.0.0.1:${state.port}`
                : 'Starting…'
              : 'Off — agents cannot connect'}
          </span>
        </div>
      </section>
      <section>
        <h3 className="mb-1 text-[12px] font-medium text-ink-2">Connect an agent</h3>
        <p className="mb-2.5 text-[12.5px] text-ink-2">
          Kawruh writes itself into the agent's own configuration — its other
          servers stay as they are. The agent picks the connection up on its
          next start.
        </p>
        <div className="grid grid-cols-1 gap-2">
          {regs.map((r) => (
            <div
              key={r.id}
              className="flex items-center justify-between gap-2 rounded-lg p-3 shadow-[inset_0_0_0_1px_var(--line)]"
            >
              <div className="min-w-0">
                <span className="text-[13px] font-medium text-ink">{r.label}</span>
                <span className="block truncate text-[11px] text-ink-3">
                  {!r.available
                    ? `Not found — the ${r.id} CLI isn't installed`
                    : r.connected
                      ? 'Connected to this Kawruh'
                      : 'Not connected'}
                </span>
              </div>
              {r.available && (
                <Button
                  type="button"
                  variant={r.connected ? 'ghost' : 'primary'}
                  disabled={busy === r.id || !state?.running}
                  onClick={() => register(r.id, !r.connected)}
                >
                  {r.connected ? 'Disconnect' : 'Connect'}
                </Button>
              )}
            </div>
          ))}
          {regs.length === 0 && <p className="text-[12.5px] text-ink-3">Looking for agents…</p>}
        </div>
        {!state?.running && (
          <p className="mt-2 text-[11.5px] text-ink-3">
            Turn the endpoint on to connect an agent.
          </p>
        )}
      </section>
      {error && (
        <p className="text-[12.5px] text-danger" role="alert">
          {error}
        </p>
      )}
    </div>
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
  const { accounts, refresh, fail, toast } = useApp()
  const [adding, setAdding] = useState<null | { serverUrl?: string; username?: string }>(
    accounts.length === 0 ? {} : null
  )
  const [invite, setInvite] = useState<{ accountId: string; code: string } | null>(null)

  const makeInvite = async (accountId: string) => {
    try {
      const { code } = await api.inviteCreate(accountId)
      setInvite({ accountId, code })
    } catch (e) {
      fail(e)
    }
  }

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
              {a.loggedIn && (
                <Button variant="ghost" className="h-7" onClick={() => void makeInvite(a.id)}>
                  <UserPlus size={13} />
                  Invite
                </Button>
              )}
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
      {invite && (
        <div className="rounded-lg bg-accent-soft p-3 text-[12px] text-ink-2">
          <div className="flex items-center gap-2">
            <code className="selectable flex-1 font-mono text-[14px] font-medium tracking-wide text-ink">
              {invite.code}
            </code>
            <IconButton
              label="Copy invite code"
              onClick={() => {
                copyText(invite.code)
                toast('Invite code copied')
              }}
            >
              <Copy size={14} />
            </IconButton>
          </div>
          <p className="mt-1.5 leading-relaxed">
            One person can use it within 7 days: in Kawruh they choose <b>Create an account</b> on{' '}
            {host(accounts.find((a) => a.id === invite.accountId)?.serverUrl ?? '')} and enter this code.
          </p>
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
  const update = useApp((s) => s.update)
  const checkForUpdate = useApp((s) => s.checkForUpdate)
  const applyUpdate = useApp((s) => s.applyUpdate)
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
      <div className="flex items-center gap-3">
        {update.kind === 'available' ? (
          <Button variant="primary" onClick={() => void applyUpdate()}>
            Restart to update to {update.version}
          </Button>
        ) : (
          <Button
            disabled={update.kind === 'checking' || update.kind === 'installing'}
            onClick={() => void checkForUpdate(true)}
          >
            {update.kind === 'checking' ? 'Checking…' : 'Check for updates'}
          </Button>
        )}
        {update.kind === 'installing' && (
          <span className="text-[12px] text-ink-3">
            Updating{update.percent === null ? '…' : ` ${update.percent}%`}
          </span>
        )}
      </div>
      {info && (
        <p className="text-[12px] text-ink-3">
          Data folder: <code className="selectable font-mono">{info.dataDir}</code>
        </p>
      )}
    </div>
  )
}
