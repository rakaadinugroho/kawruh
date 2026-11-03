#!/usr/bin/env node
// End-to-end check of Assist (the AI writing pane) against a debug build
// running with KAWRUH_E2E_PORT. It drives a real installed agent CLI
// (claude / opencode / agy), so each ask takes tens of seconds.
//
//   cd app && KAWRUH_E2E_PORT=4799 KAWRUH_DATA_DIR=$(mktemp -d) pnpm tauri dev
//   node app/tests/e2e/assist.mjs [--port 4799]
//
// Checks: agents_list finds an agent; ⌘J on a selection opens the panel and
// shows what the agent sees; the proposal shows and Apply lands it in the
// page; ⌘J on a caret drafts and inserts after the block. Skips (exits 0)
// when no agent CLI is installed on this computer — the feature is optional
// by design.

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, v, i, all) => (v.startsWith('--') ? [...acc, [v.slice(2), all[i + 1]]] : acc), [])
)
const PORT = Number(args.port ?? 4799)
const run = Math.random().toString(36).slice(2, 8)

async function ev(port, js) {
  const res = await fetch(`http://127.0.0.1:${port}/eval`, { method: 'POST', body: js })
  const { ok, value } = await res.json()
  if (!ok) throw new Error(`eval on ${port} failed: ${value}`)
  return JSON.parse(value)
}

let failures = 0
function check(name, cond, detail = '') {
  console.log(`${cond ? 'PASS' : 'FAIL'}  ${name}${detail ? `  (${detail})` : ''}`)
  if (!cond) failures++
}

const helpers = `
  const wait = (ms) => new Promise(r => setTimeout(r, ms));
  const inv = window.__TAURI_INTERNALS__.invoke;
  const until = async (fn, ms = 15000) => { const t = Date.now(); while (Date.now() - t < ms) { const v = await fn(); if (v) return v; await wait(200); } return null; };
  const rows = () => [...document.querySelectorAll('[data-testid=tree-row]')];
  const editorEl = () => document.querySelector('.kw-editor .ProseMirror');
  const editorText = () => editorEl()?.innerText ?? '';
  /// Selects a paragraph's whole contents so PM sees a range (the same way a
  /// human selection registers): set the DOM selection, then make the editor
  /// read it through the selectionchange flush.
  const selectParagraph = async (needle) => {
    const pm = editorEl(); if (!pm) return { ok: false, why: 'no editor' };
    pm.focus(); await wait(100);
    const p = [...pm.querySelectorAll('p')].find(el => el.innerText.includes(needle));
    if (!p) return { ok: false, why: 'no paragraph with: ' + needle };
    const range = document.createRange(); range.selectNodeContents(p);
    const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range);
    await wait(150); document.dispatchEvent(new Event('selectionchange')); await wait(150);
    return { ok: true };
  };
  /// ⌘J on document.body bubbles to the editor's window keydown handler; PM
  /// itself never sees the key (its handlers sit on editor children).
  const keyJ = () => document.body.dispatchEvent(
    new KeyboardEvent('keydown', { key: 'j', metaKey: true, bubbles: true, cancelable: true }));
  /// Fills the panel's instruction the React way (value setter + input event).
  const fillInstruction = (text) => {
    const input = document.querySelector('[data-testid=assist-panel] input[aria-label="Instruction"]');
    if (!input) return false;
    Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
    return true;
  };
  const submitPanel = () => {
    const btn = document.querySelector('[data-testid=assist-panel] button[type=submit]');
    if (!btn) return false; btn.click(); return true;
  };
  /// Waits for the agent's answer: the Apply button, or the panel's error
  /// paragraph (so a broken agent fails the check with its message, not a
  /// slow timeout). Returns {reply:'apply'} | {reply:'error', message} | null.
  const awaitReply = async (ms) => until(() => {
    const pane = document.querySelector('[data-testid=assist-panel]');
    const err = pane?.querySelector('p.text-danger');
    if (err?.innerText.trim()) return { reply: 'error', message: err.innerText.trim() };
    if (pane?.querySelector('[data-testid=assist-apply]')) return { reply: 'apply' };
    return null;
  }, ms)
  const panelClosed = () => until(() => !document.querySelector('[data-testid=assist-panel]'), 20000)
`

async function openWorkspace(wsId) {
  await ev(PORT, `${helpers} await inv('setting_set', { key: 'lastWorkspace', value: '${wsId}' }); setTimeout(() => location.reload(), 50); return 1`)
  await new Promise((r) => setTimeout(r, 3000))
}

console.log(`Assist e2e, run ${run}, port ${PORT}`)

// 1. Which agents this computer offers — none means skip, not fail.
const agents = await ev(PORT, `${helpers} return inv('agents_list')`)
const available = agents.filter((a) => a.available)
if (available.length === 0) {
  console.log(`SKIP  no agent CLI found (looked for: ${agents.map((a) => a.id).join(', ') || 'none'})`)
  process.exit(0)
}
check('agents_list finds an installed agent', true, `${available.map((a) => a.id).join(' + ')}`)

// 2. A throwaway workspace with one seeded page.
const ws = await ev(PORT, `${helpers} return inv('workspace_create', { name: 'Assist E2E ${run}', accountId: null })`)
check('workspace_created', !!ws?.id)
const seedTitle = `Assist ${run}`
const seedLine = 'The quick brown fox jumps over the lazy dog.'
await ev(PORT, `${helpers} return inv('page_import_markdown', { workspaceId: '${ws.id}', parent: '', title: '${seedTitle}', markdownText: '${seedLine}\\n\\nA second plain paragraph for structure.' })`)
await openWorkspace(ws.id)
await ev(PORT, `${helpers}
  await until(() => rows().some(r => r.innerText.trim().startsWith('${seedTitle}')));
  rows().find(r => r.innerText.trim().startsWith('${seedTitle}')).click();
  return !!(await until(() => editorEl()));`)

// 3. ⌘J with a selection: the panel opens and shows what the agent sees.
{
  const sel = await ev(PORT, `${helpers} return selectParagraph('quick brown fox')`)
  check('the seed paragraph got selected in the DOM', sel.ok, sel.why)
  await ev(PORT, `${helpers} keyJ(); return 1`)
  const pane = await ev(PORT, `${helpers} return until(() => !!document.querySelector('[data-testid=assist-panel]'))`)
  check('⌘J opens the Assist panel', !!pane)
  const sees = await ev(PORT, `${helpers}
    const pane = document.querySelector('[data-testid=assist-panel]');
    const pre = pane?.querySelector('pre');
    return { text: pre?.innerText ?? '', title: pane?.querySelector('h2')?.innerText ?? pane?.querySelector('header')?.innerText ?? '' }`)
  check('the panel shows what the agent sees', (sees.text || '').includes('quick brown fox'),
    JSON.stringify(sees.text.slice(0, 60)))
}

// 4. Ask → the agent answers → the diff shows → Apply puts it on the page.
const repl = await ev(PORT, `${helpers}
  if (!fillInstruction('Rewrite this exact sentence as "The exceedingly swift auburn vixen vaults above the torpid hound."')) return { no: 'input' }
  if (!submitPanel()) return { no: 'submit' }
  return awaitReply(240000)`)
check('the agent answered a rewrite', repl?.reply === 'apply', repl?.message || repl?.no || '')
if (repl.reply !== 'apply') {
  await ev(PORT, `${helpers} return inv('workspace_forget', { workspaceId: '${ws.id}' })`)
  process.exit(1)
}
const proposalShown = await ev(PORT, `${helpers}
  const pane = document.querySelector('[data-testid=assist-panel]');
  const pre = pane?.querySelector('[data-testid=assist-proposal]');
  return pre?.innerText ?? ''`)
check('the panel previews what would change', (proposalShown || '').includes('exceedingly'))
const apply1 = await ev(PORT, `${helpers}
  document.querySelector('[data-testid=assist-apply]').click();
  return !!await panelClosed()`)
check('Apply closes the panel', !!apply1)
await new Promise((r) => setTimeout(r, 600))
const after = await ev(PORT, `${helpers} return editorText()`)
check('Apply rewrote the paragraph on the page', !after.includes(seedLine) && after.includes('exceedingly swift'),
  JSON.stringify(after.slice(0, 80)))

// 5. ⌘J on a caret: the draft panel, inserted after the current block.
const mid = await ev(PORT, `${helpers}
  const pm = editorEl(); pm.focus();
  const p = [...pm.querySelectorAll('p')].at(-1) ?? pm; p.click(); await wait(200);
  keyJ();
  const pane = await until(() => document.querySelector('[data-testid=assist-panel]'));
  return !!pane`)
check('⌘J on a caret opens the draft panel', !!mid)
const draft = await ev(PORT, `${helpers}
  if (!fillInstruction('Draft exactly this line: "A cheerful cup of coffee waits below."')) return { no: 'input' }
  if (!submitPanel()) return { no: 'submit' }
  return awaitReply(240000)`)
check('the agent answered a draft', draft?.reply === 'apply', draft?.message || draft?.no || '')
if (draft.reply !== 'apply') {
  await ev(PORT, `${helpers} return inv('workspace_forget', { workspaceId: '${ws.id}' })`)
  process.exit(1)
}
const apply2 = await ev(PORT, `${helpers}
  document.querySelector('[data-testid=assist-apply]').click();
  return !!await panelClosed()`)
check('Apply closes the draft panel', !!apply2)
await new Promise((r) => setTimeout(r, 600))
const finalText = await ev(PORT, `${helpers} return editorText()`)
check('Apply inserted the draft below the block', finalText.includes('cheerful cup of coffee'))

// 6. Done: the workspace disappears again.
await ev(PORT, `${helpers} return inv('workspace_forget', { workspaceId: '${ws.id}' })`)
console.log(failures ? `${failures} FAILED` : 'ALL PASS')
process.exit(failures ? 1 : 0)
