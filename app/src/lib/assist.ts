/// AI assist: how a request becomes a prompt, how an answer is cleaned up,
/// and how the panel shows what would change. The agents themselves are the
/// CLIs on this computer (no API keys in Kawruh — Settings → Assist).
import { useEffect, useState } from 'react'
import { type AgentInfo, api } from './ipc'

/// Settings key: the agent that answers Assist by default.
export const ASSIST_AGENT = 'assist_agent'

/// Fetches the installed agents and the chosen default. When nothing was
/// chosen (first use), the first available agent is picked and remembered.
export function useAssistAgent(): {
  agents: AgentInfo[]
  chosen: AgentInfo | null
  choose: (id: string) => void
} {
  const [agents, setAgents] = useState<AgentInfo[]>([])
  const [pick, setPick] = useState('')
  useEffect(() => {
    let live = true
    void Promise.all([api.agentsList(), api.settingGet(ASSIST_AGENT)])
      .then(([list, saved]) => {
        if (!live) return
        setAgents(list)
        const effective =
          saved && list.some((a) => a.id === saved && a.available)
            ? saved
            : (list.find((a) => a.available)?.id ?? '')
        setPick(effective)
        if (effective && saved !== effective) void api.settingSet(ASSIST_AGENT, effective)
      })
      .catch(() => {})
    return () => {
      live = false
    }
  }, [])
  const choose = (id: string) => {
    setPick(id)
    void api.settingSet(ASSIST_AGENT, id)
  }
  return { agents, chosen: agents.find((a) => a.id === pick && a.available) ?? null, choose }
}

// -------------------------------------------------- prompts

const SEL = '<<<KAWRUH_SELECTED>>>'
const SEL_END = '<<<END_SELECTED>>>'
const PAGE = '<<<KAWRUH_PAGE>>>'
const PAGE_END = '<<<END_PAGE>>>'
const AFTER = '<<<INSERT_AFTER>>>'
const AFTER_END = '<<<END_INSERT_AFTER>>>'

/// Long pages are truncated for the prompt; the selection is always whole.
const PAGE_CAP = 12_000
const capPage = (page: string) =>
  page.length <= PAGE_CAP ? page : `${page.slice(0, PAGE_CAP)}\n[… page cut off for length …]`

/// All prompts start with "You are", so the agent CLI can never mistake the
/// beginning of the prompt for one of its own flags (agents.rs).
export function rewritePrompt(instruction: string, selection: string, page: string): string {
  return `You are the writing assistant inside Kawruh, a markdown notebook. The user selected part of a page and wants it edited. Kawruh will replace the selected text with your reply.

Selected text:
${SEL}
${selection}
${SEL_END}

The page the selection is part of, for context only:
${PAGE}
${capPage(page)}
${PAGE_END}

What to do with the selection: ${instruction.trim() || 'Improve the wording.'}

A page can hold kawruh://page/<id> links, kawruh-asset://… images and \`\`\`mermaid / \`\`\`kawruh-canvas fences — keep every such reference the selection carries, and keep what makes sense as markdown (lists stay lists, headings stay headings).
Do not use any tools or load skills; reply directly.
Reply with the replacement markdown only; no explanations, and don't wrap the whole reply in a code fence.`
}

export function draftPrompt(instruction: string, after: string, page: string): string {
  return `You are the writing assistant inside Kawruh, a markdown notebook. The user asks you to draft a piece for a page; Kawruh will insert your reply at the spot below.

Task: ${instruction.trim() || 'Draft something useful for this page.'}

The page the piece becomes part of:
${PAGE}
${capPage(page)}
${PAGE_END}

It is inserted right after this block (empty means an empty spot in the page):
${AFTER}
${after}
${AFTER_END}

Match the page's voice and level of detail. A page can hold kawruh://page/<id> links, kawruh-asset://… images and \`\`\`mermaid / \`\`\`kawruh-canvas fences — use plain markdown otherwise.
Do not use any tools or load skills; reply directly.
Reply with only the markdown to insert; no explanations, and don't wrap the whole reply in a code fence.`
}

// -------------------------------------------------- answers

/// Agents sometimes hand back the markdown inside a code fence. That wrap
/// is an artifact; a fence with a real language (mermaid, kawruh-canvas) is
/// content and stays.
export function stripFences(md: string): string {
  const trimmed = md.trim()
  const wrap = /^````?\s*(\w*)[^\S\n]*\n([\s\S]*)\n````?\s*$/.exec(trimmed)
  if (!wrap) return trimmed
  const lang = wrap[1].toLowerCase()
  if (lang === '' || lang === 'markdown' || lang === 'md') return wrap[2]
  return trimmed
}

// -------------------------------------------------- diff

export type DiffLine = { kind: 'same' | 'del' | 'add'; text: string }

/// Line diff of the original against the proposal, longest common
/// subsequence style. Very long sides skip the diff and show as one change.
export function lineDiff(oldText: string, text: string): DiffLine[] {
  const a = oldText.split('\n')
  const b = text.split('\n')
  const MAX = 1500
  if (a.length > MAX || b.length > MAX) {
    return [
      ...a.map((t): DiffLine => ({ kind: 'del', text: t })),
      ...b.map((t): DiffLine => ({ kind: 'add', text: t })),
    ]
  }
  const m = a.length
  const n = b.length
  const w = n + 1
  // lcs[i][j]: how many lines of a[i..] and b[j..] match.
  const lcs = new Uint32Array((m + 1) * w)
  for (let i = m - 1; i >= 0; i--) {
    for (let j = n - 1; j >= 0; j--) {
      lcs[i * w + j] =
        a[i] === b[j]
          ? lcs[(i + 1) * w + j + 1] + 1
          : Math.max(lcs[(i + 1) * w + j], lcs[i * w + j + 1])
    }
  }
  const out: DiffLine[] = []
  let i = 0
  let j = 0
  while (i < m && j < n) {
    if (a[i] === b[j]) {
      out.push({ kind: 'same', text: a[i] })
      i++
      j++
    } else if (lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
      out.push({ kind: 'del', text: a[i] })
      i++
    } else {
      out.push({ kind: 'add', text: b[j] })
      j++
    }
  }
  while (i < m) out.push({ kind: 'del', text: a[i++] })
  while (j < n) out.push({ kind: 'add', text: b[j++] })
  return out
}
