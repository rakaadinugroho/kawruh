import { useState } from 'react'
import { type Account, api, errorMessage } from '../lib/ipc'
import { useApp } from '../lib/store'
import { Button, Field, inputClass } from './ui'

export const DEFAULT_SERVER = (import.meta.env.VITE_DEFAULT_SERVER as string | undefined) ?? ''

/// Log in to (or create an account on) a Kawruh server.
export function AccountForm({
  initial,
  onDone,
  onCancel,
}: {
  initial?: { serverUrl?: string; username?: string }
  onDone: (a: Account) => void
  onCancel?: () => void
}) {
  const refresh = useApp((s) => s.refresh)
  const [register, setRegister] = useState(false)
  const [serverUrl, setServerUrl] = useState(initial?.serverUrl ?? DEFAULT_SERVER)
  const [username, setUsername] = useState(initial?.username ?? '')
  const [password, setPassword] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const account = await api.login({
        serverUrl,
        username: username.trim().toLowerCase(),
        password,
        register,
        displayName: register ? displayName : undefined,
      })
      await refresh()
      onDone(account)
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-3" data-testid="account-form">
      <Field label="Server">
        <input
          className={inputClass}
          value={serverUrl}
          onChange={(e) => setServerUrl(e.target.value)}
          placeholder="http://192.168.1.10:7420"
          required
          name="server"
        />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Username">
          <input
            className={inputClass}
            value={username}
            onChange={(e) => setUsername(e.target.value)}
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            required
            minLength={3}
            name="username"
          />
        </Field>
        <Field label="Password">
          <input
            className={inputClass}
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            required
            minLength={register ? 8 : 1}
            name="password"
          />
        </Field>
      </div>
      {register && (
        <Field label="Display name" hint="Shown next to your cursor when you edit together.">
          <input
            className={inputClass}
            value={displayName}
            onChange={(e) => setDisplayName(e.target.value)}
            name="displayName"
          />
        </Field>
      )}
      {error && <p className="text-[12px] text-danger" role="alert">{error}</p>}
      <div className="mt-1 flex items-center justify-between gap-2">
        <button
          type="button"
          className="text-[12px] text-accent hover:underline"
          onClick={() => {
            setRegister(!register)
            setError(null)
          }}
        >
          {register ? 'I already have an account' : 'Create an account instead'}
        </button>
        <div className="flex gap-2">
          {onCancel && (
            <Button variant="ghost" onClick={onCancel}>
              Cancel
            </Button>
          )}
          <Button variant="primary" type="submit" disabled={busy}>
            {busy ? 'Connecting…' : register ? 'Create account' : 'Log in'}
          </Button>
        </div>
      </div>
    </form>
  )
}
