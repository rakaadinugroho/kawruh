import { ArrowRight, Laptop, Server } from 'lucide-react'
import { useState } from 'react'
import { api } from '../lib/ipc'
import { useApp } from '../lib/store'
import { thisDevice } from '../lib/platform'
import { AccountForm } from './AccountForm'

/// First run: no workspace yet.
export function Onboarding() {
  const { refresh, selectWorkspace, setDialog, fail } = useApp()
  const [connecting, setConnecting] = useState(false)
  const [busy, setBusy] = useState(false)

  const startLocal = async () => {
    setBusy(true)
    try {
      const ws = await api.createWorkspace('My notes', null)
      await refresh()
      selectWorkspace(ws.id)
    } catch (e) {
      fail(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex h-full flex-col bg-paper" data-testid="onboarding">
      <div data-tauri-drag-region className="h-11 shrink-0" />
      <div className="flex flex-1 items-center overflow-y-auto px-[12vw] pb-16">
        <div className="w-full max-w-[520px]">
          <img src="/icon.png" alt="" width={56} height={56} className="mb-6 rounded-[13px]" />
          <h1 className="font-serif text-[44px] leading-[1.05] font-[650] tracking-[-0.022em] [font-variation-settings:'opsz'_72]">
            Kawruh
          </h1>
          <p className="mt-3 max-w-[440px] font-serif text-[18px] leading-relaxed text-ink-2">
            A markdown notebook you own. Write on {thisDevice}, nest pages as deep as you like, and share
            through your own server when you're ready.
          </p>

          {connecting ? (
            <div className="mt-8 rounded-lg bg-raised p-5 shadow-pop">
              <AccountForm
                onDone={() => setDialog({ kind: 'join' })}
                onCancel={() => setConnecting(false)}
              />
            </div>
          ) : (
            <div className="mt-8 flex flex-col gap-2">
              <button
                type="button"
                onClick={startLocal}
                disabled={busy}
                data-testid="start-local"
                className="press group flex items-center gap-3 rounded-lg bg-accent px-4 py-3 text-left text-accent-ink"
              >
                <Laptop size={18} />
                <span className="flex-1">
                  <span className="block text-[14px] font-semibold">Start writing on {thisDevice}</span>
                  <span className="block text-[12px] opacity-80">No account needed. Publish later.</span>
                </span>
                <ArrowRight size={16} className="transition-transform duration-150 group-hover:translate-x-0.5" />
              </button>
              <button
                type="button"
                onClick={() => setConnecting(true)}
                className="press flex items-center gap-3 rounded-lg px-4 py-3 text-left text-ink shadow-[inset_0_0_0_1px_var(--line)] hover:bg-hover"
              >
                <Server size={18} className="text-ink-2" />
                <span className="flex-1">
                  <span className="block text-[14px] font-semibold">Connect to a Kawruh server</span>
                  <span className="block text-[12px] text-ink-3">Join workspaces shared with you.</span>
                </span>
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
