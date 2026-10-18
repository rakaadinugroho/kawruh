#!/usr/bin/env node
// End-to-end collaboration check against a real Kawruh server, driving two
// running debug builds of the app through their E2E hooks (KAWRUH_E2E_PORT).
//
//   KAWRUH_E2E_PORT=4797 KAWRUH_DATA_DIR=/tmp/a pnpm tauri dev      # instance A
//   KAWRUH_E2E_PORT=4798 KAWRUH_DATA_DIR=/tmp/b ./target/debug/kawruh # instance B
//   node app/tests/e2e/collab.mjs --server http://<host>:7420 [--a 4797 --b 4798]
//
// Creates two throwaway accounts (printed at the end so they can be removed
// with `deploy/admin.sh delete-user <name>`), a fresh server workspace, and
// checks: sync on attach, live typing both ways, presence, MCP edits arriving
// live, and the public share page.

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, v, i, all) => (v.startsWith('--') ? [...acc, [v.slice(2), all[i + 1]]] : acc), [])
)
const SERVER = args.server ?? 'http://127.0.0.1:7420'
const A = Number(args.a ?? 4797)
const B = Number(args.b ?? 4798)
const run = Math.random().toString(36).slice(2, 8)
const password = `e2e-${run}-${Math.random().toString(36).slice(2, 10)}`

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
  const editorText = () => document.querySelector('.kw-editor .ProseMirror')?.innerText ?? '';
  const typeAtEnd = async (text) => {
    const pm = document.querySelector('.kw-editor .ProseMirror'); pm.focus();
    const blocks = pm.querySelectorAll('p'); const p = blocks[blocks.length - 1] ?? pm;
    const walker = document.createTreeWalker(p, NodeFilter.SHOW_TEXT, { acceptNode: n => n.parentElement.closest('.ProseMirror-yjs-cursor') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT });
    let last = null; while (walker.nextNode()) last = walker.currentNode;
    const range = document.createRange();
    if (last) range.setStart(last, last.length); else range.selectNodeContents(p);
    range.collapse(false);
    const sel = getSelection(); sel.removeAllRanges(); sel.addRange(range);
    await wait(100); document.execCommand('insertText', false, text); await wait(300);
  };
`

async function openWorkspace(port, wsId) {
  await ev(port, `${helpers} await inv('setting_set', { key: 'lastWorkspace', value: '${wsId}' }); setTimeout(() => location.reload(), 50); return 1`)
  await new Promise((r) => setTimeout(r, 3000))
}

async function openPage(port, title) {
  return ev(
    port,
    `${helpers}
     await until(() => rows().some(r => r.innerText.trim().startsWith(${JSON.stringify(title)})));
     rows().find(r => r.innerText.trim().startsWith(${JSON.stringify(title)})).click();
     return !!(await until(() => document.querySelector('.kw-editor .ProseMirror')));`
  )
}

console.log(`Server ${SERVER}, instances ${A} and ${B}, run ${run}`)
const alice = `e2e-a-${run}`
const bob = `e2e-b-${run}`

// 1. Accounts + a fresh shared workspace.
const ws = await ev(
  A,
  `${helpers}
   const acc = await inv('account_login', { serverUrl: '${SERVER}', username: '${alice}', password: '${password}', register: true, displayName: 'Alice ${run}' });
   const ws = await inv('workspace_create', { name: 'E2E ${run}', accountId: acc.id });
   await inv('member_add', { workspaceId: ws.id, username: '${bob}', role: 'editor' }).catch(() => null);
   return { id: ws.id, accountId: acc.id };`
).catch(async (e) => {
  throw new Error(`setting up instance A failed: ${e.message}`)
})
await ev(
  B,
  `${helpers} await inv('account_login', { serverUrl: '${SERVER}', username: '${bob}', password: '${password}', register: true, displayName: 'Bob ${run}' }); return 1`
)
// Bob exists now; (re)invite and attach.
await ev(A, `${helpers} await inv('member_add', { workspaceId: '${ws.id}', username: '${bob}', role: 'editor' }); return 1`)
const attached = await ev(
  B,
  `${helpers} const acc = (await inv('accounts_list')).find(a => a.username === '${bob}'); const w = await inv('workspace_attach', { accountId: acc.id, workspaceId: '${ws.id}' }); return w.role`
)
check('bob attached as editor', attached === 'editor', attached)

await openWorkspace(A, ws.id)
await openWorkspace(B, ws.id)
check('A opened welcome page', await openPage(A, 'Welcome to Kawruh'))
check('B received the workspace from the server', await openPage(B, 'Welcome to Kawruh'))

// 2. Live typing both ways.
const markA = `alice-${run}`
const markB = `bob-${run}`
await ev(A, `${helpers} await typeAtEnd(' ${markA}'); return 1`)
check('A → B live edit', await ev(B, `${helpers} return !!(await until(() => editorText().includes('${markA}')))`))
await ev(B, `${helpers} await typeAtEnd(' ${markB}'); return 1`)
check('B → A live edit', await ev(A, `${helpers} return !!(await until(() => editorText().includes('${markB}')))`))

// 3. Presence.
const peersOnA = await ev(
  A,
  `${helpers} return await until(() => [...document.querySelectorAll('[data-testid=presence] span[title]')].map(s => s.title).join(',') || null)`
)
check('A sees Bob present', (peersOnA ?? '').includes('Bob'), peersOnA ?? 'none')

// 4. MCP: an agent edit arrives live in both editors.
const token = await ev(A, `${helpers} return (await inv('token_create', { accountId: '${ws.accountId}', name: 'e2e ${run}' })).token`)
const pageId = await ev(A, `${helpers} return rows().find(r => r.innerText.trim().startsWith('Welcome')).dataset.pageId`)
const mcp = async (body, session) => {
  const res = await fetch(`${SERVER}/mcp`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json',
      Accept: 'application/json, text/event-stream',
      ...(session ? { 'Mcp-Session-Id': session } : {}),
    },
    body: JSON.stringify(body),
  })
  const text = await res.text()
  const data = text.split('\n').find((l) => l.startsWith('data: {'))
  return { session: res.headers.get('mcp-session-id'), json: data ? JSON.parse(data.slice(6)) : null, status: res.status }
}
const init = await mcp({
  jsonrpc: '2.0',
  id: 1,
  method: 'initialize',
  params: { protocolVersion: '2025-06-18', capabilities: {}, clientInfo: { name: 'kawruh-e2e', version: '1' } },
})
check('MCP initialize', !!init.session && init.json?.result?.serverInfo?.name === 'kawruh')
await mcp({ jsonrpc: '2.0', method: 'notifications/initialized' }, init.session)
const agentMark = `agent-${run}`
const upd = await mcp(
  {
    jsonrpc: '2.0',
    id: 2,
    method: 'tools/call',
    params: { name: 'update_page', arguments: { page_id: pageId, mode: 'append', markdown: `## ${agentMark}\n\n- [x] arrived live` } },
  },
  init.session
)
check('MCP update_page', upd.json?.result?.isError === false)
for (const [port, who] of [[A, 'A'], [B, 'B']]) {
  check(`agent edit live on ${who}`, await ev(port, `${helpers} return !!(await until(() => editorText().includes('${agentMark}')))`))
}
const read = await mcp(
  { jsonrpc: '2.0', id: 3, method: 'tools/call', params: { name: 'read_page', arguments: { page_id: pageId } } },
  init.session
)
const md = read.json?.result?.content?.[0]?.text ?? ''
check('MCP read_page sees human edits', md.includes(markA) && md.includes(markB))

// 5. Public share link.
const share = await ev(A, `${helpers} return (await inv('share_create', { workspaceId: '${ws.id}', pageId: '${pageId}' })).url`)
const html = await (await fetch(share)).text()
check('share page renders content', html.includes(agentMark) && html.includes(markA), share)

console.log(`\n${failures === 0 ? 'ALL PASSED' : `${failures} FAILED`}`)
console.log(`Throwaway accounts: ${alice} ${bob} (remove with deploy/admin.sh delete-user <name>)`)
process.exit(failures === 0 ? 0 : 1)
