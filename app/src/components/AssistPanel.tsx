/// The Assist window: picks an agent (the CLIs on this computer), asks it,
/// shows what it would change, and only reaches the page on Apply. Better yet
/// the human approves every piece of text the agent writes.
import { Check, Loader2, RotateCcw, Square, X } from 'lucide-react'
import { Channel } from '@tauri-apps/api/core'
import { useEffect, useMemo, useRef, useState } from 'react'
import {
  lineDiff,
  rewritePrompt,
  draftPrompt,
  stripFences,
  useAssistAgent,
} from '../lib/assist'
import { type AssistEvent, api } from '../lib/ipc'
import { Button, Dialog, inputClass } from './ui'

/// What the editor handed over when assist was opened, so Apply knows where
/// the change goes and whether the page is still where it was.
export type AssistSession =
  | { mode: 'rewrite'; range: [number, number]; anchor: string; original: string }
  | { mode: 'draft'; range: [number, number]; replaceBlock: boolean; after: string }

const PRESETS = [
  'Improve the wording',
  'Make it shorter',
  'Make it longer',
  'Summarise in a bullet list',
]

/// How steep the shown diff may get before we tell the user to open the page
/// and read the result there instead.
const SHOWN_DIFF = 240

export function AssistPanel({ session, workspaceId, pageId, onClose, onApply }: {
  session: AssistSession
  workspaceId: string
  pageId: string
  onClose: () => void
  /// Called with the agent's markdown; the editor applies it (or fails and
  /// keeps the panel open) and closes the panel on success.
  onApply: (markdown: string) => void
}) {
  const { agents, chosen } = useAssistAgent()
  const [instruction, setInstruction] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [text, setText] = useState<string | null>(null)
  const runId = useRef<string | null>(null)
  const instructionRef = useRef(instruction)
  instructionRef.current = instruction
  const pageCache = useRef<string | null>(null)
  const rewrite = session.mode === 'rewrite'

  // Leaving the panel (close, unmount, escape) stops the agent.
  useEffect(
    () => () => {
      if (runId.current) void api.assistCancel(runId.current).catch(() => {})
    },
    [],
  )

  const stop = () => {
    if (runId.current) void api.assistCancel(runId.current).catch(() => {})
    runId.current = null
    setBusy(false)
  }

  const ask = (task: string) => {
    if (!chosen || busy) return
    setBusy(true)
    setError(null)
    setText(null)
    void (async () => {
      pageCache.current ??= await api.exportMarkdown(workspaceId, pageId).catch(() => '')
      const prompt =
        rewrite
          ? rewritePrompt(task, session.original, pageCache.current ?? '')
          : draftPrompt(task, session.after, pageCache.current ?? '')
      const channel = new Channel<AssistEvent>()
      channel.onmessage = (event) => {
        if (event.type === 'text') {
          runId.current = null
          setText(event.text)
          setBusy(false)
        } else if (event.type === 'error') {
          runId.current = null
          setError(event.message)
          setBusy(false)
        }
      }
      try {
        runId.current = await api.assistRun(chosen.id, prompt, channel)
      } catch (e) {
        runId.current = null
        setBusy(false)
        setError(e instanceof Error ? e.message : String(e))
      }
    })()
  }

  const noAgent = agents.length === 0 || !agents.some((a) => a.available)
  const proposal = text === null ? null : stripFences(text)
  const diff = useMemo(
    () => (rewrite && proposal ? lineDiff(session.original, proposal) : []),
    [rewrite, proposal, session],
  )
  const shownDiff = diff.length > SHOWN_DIFF ? diff.slice(0, SHOWN_DIFF) : diff

  return (
    <Dialog
      open
      onClose={busy ? () => {} : onClose}
      width={640}
      title={rewrite ? 'Assist — rewrite the selection' : 'Assist — draft here'}
      testId="assist-panel"
    >
      {noAgent ? (
        <div className="flex flex-col gap-3 pb-1">
          <p className="text-[13px] text-ink-2">
            Assist runs the AI agents already on this computer — no account or
            API key inside Kawruh. None was found just now.
          </p>
          <ul className="list-inside list-disc text-[13px] text-ink-2">
            <li><code className="font-mono text-ink">claude</code> — Claude Code</li>
            <li><code className="font-mono text-ink">agy</code> — Antigravity CLI</li>
            <li><code className="font-mono text-ink">opencode</code> — OpenCode</li>
          </ul>
          <p className="text-[12px] text-ink-3">
            Install one, restart Kawruh, and it appears in Settings → Assist.
          </p>
        </div>
      ) : (
        <div className="flex flex-col gap-4">
          <p className="-mt-1 text-[12px] text-ink-3">
            With {rewrite ? 'the text selected' : 'the cursor where it is'}, {chosen?.label} gets
            the page as context and answers with markdown; nothing changes until you press Apply.
          </p>
          {!busy && text === null && !error && (
            <>
              {rewrite && (
                <section>
                  <h3 className="mb-1.5 text-[12px] font-medium text-ink-2">Quick asks</h3>
                  <div className="flex flex-wrap gap-1.5">
                    {PRESETS.map((preset) => (
                      <button
                        key={preset}
                        type="button"
                        onClick={() => {
                          setInstruction(preset)
                          void ask(preset)
                        }}
                        className="press h-7 rounded-md bg-hover px-2.5 text-[12px] text-ink-2 hover:bg-hover-2 hover:text-ink"
                      >
                        {preset}
                      </button>
                    ))}
                  </div>
                </section>
              )}
              <form
                className="flex gap-2"
                onSubmit={(e) => {
                  e.preventDefault()
                  void ask(instruction)
                }}
              >
                <input
                  autoFocus
                  className={inputClass}
                  style={{ flex: 1 }}
                  placeholder={rewrite ? 'How should the selection change?' : 'What should I draft?'}
                  value={instruction}
                  onChange={(e) => setInstruction(e.target.value)}
                  aria-label="Instruction"
                />
                <Button variant="primary" type="submit" disabled={!instruction.trim()}>
                  {rewrite ? 'Rewrite' : 'Draft'}
                </Button>
              </form>
              {rewrite && (
                <section>
                  <h3 className="mb-1 text-[12px] font-medium text-ink-2">What the agent sees</h3>
                  <pre className="max-h-32 overflow-auto rounded-md bg-paper p-2 font-mono text-[11.5px] leading-[1.5] text-ink-3 shadow-[inset_0_0_0_1px_var(--line)]">
                    {session.original || '(empty selection — nothing to show)'}
                  </pre>
                </section>
              )}
            </>
          )}
          {busy && (
            <div className="flex items-center justify-between rounded-md bg-hover px-3 py-2.5">
              <span className="flex items-center gap-2 text-[13px] text-ink-2">
                <Loader2 size={14} className="animate-spin" />
                Asking {chosen?.label}… page edits wait until Apply.
              </span>
              <Button variant="ghost" onClick={stop}>
                <Square size={12} /> Stop
              </Button>
            </div>
          )}
          {error && (
            <div className="flex flex-col gap-2">
              <p className="text-[13px] text-danger">{error}</p>
              <div className="flex justify-end gap-2">
                <Button variant="subtle" onClick={() => void ask(instructionRef.current)}>
                  <RotateCcw size={13} /> Retry
                </Button>
                <Button variant="ghost" onClick={onClose}>
                  Close
                </Button>
              </div>
            </div>
          )}
          {proposal !== null && (
            <section className="flex flex-col gap-2">
              <h3 className="text-[12px] font-medium text-ink-2">
                {rewrite ? 'What would change' : 'What would appear'}
              </h3>
              <pre
                className="max-h-72 overflow-auto rounded-md bg-paper p-2 font-mono text-[11.5px] leading-[1.55] shadow-[inset_0_0_0_1px_var(--line)]"
                data-testid="assist-proposal"
              >
                {rewrite
                  ? shownDiff.map((line, i) => (
                      <span
                        key={i}
                        className={
                          line.kind === 'add'
                            ? 'kw-assist-add'
                            : line.kind === 'del'
                              ? 'kw-assist-del'
                              : undefined
                        }
                      >
                        {line.kind === 'add' && '+ '}
                        {line.kind === 'del' && '− '}
                        {line.text}
                        {'\n'}
                      </span>
                    ))
                  : proposal}
              </pre>
              {diff.length > SHOWN_DIFF && (
                <p className="text-[11.5px] text-ink-3">… {diff.length - SHOWN_DIFF} lines more.</p>
              )}
              {busy ? null : (
                <div className="flex items-center justify-between">
                  <span className="text-[11.5px] text-ink-3">
                    {chosen?.label} wrote this; you apply it.
                  </span>
                  <div className="flex gap-2">
                    <Button variant="ghost" onClick={() => void ask(instructionRef.current)}>
                      <RotateCcw size={13} /> Ask again
                    </Button>
                    <Button variant="ghost" onClick={() => { setText(null); setError(null) }}>
                      <X size={13} /> Change ask
                    </Button>
                    <Button
                      variant="primary"
                      disabled={!proposal.trim()}
                      onClick={() => onApply(proposal)}
                      data-testid="assist-apply"
                    >
                      <Check size={13} /> Apply
                    </Button>
                  </div>
                </div>
              )}
            </section>
          )}
        </div>
      )}
    </Dialog>
  )
}
