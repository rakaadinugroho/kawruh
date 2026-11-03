#!/usr/bin/env node
// End-to-end check of the desktop's local MCP endpoint against a debug build
// running with KAWRUH_E2E_PORT. The test talks JSON-RPC to the endpoint the
// way an agent CLI would (initialize → tools → calls), then reads pages back
// through the app to prove writes landed in the live documents.
//
//   cd app && KAWRUH_E2E_PORT=4799 KAWRUH_DATA_DIR=$(mktemp -d) pnpm tauri dev
//   node app/tests/e2e/mcp.mjs [--port 4799]
//
// Nothing here touches agent registration (mcp_register): that writes real
// CLI configs on this computer and only ever runs from a user's click.

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

console.log(`MCP e2e, run ${run}, port ${PORT}`)

// 1. A throwaway workspace; the endpoint serves exactly these local ones.
const ws = await ev(
  PORT,
  `const inv = window.__TAURI_INTERNALS__.invoke; return inv('workspace_create', { name: 'MCP E2E ${run}', accountId: null })`
)
const helpers = `const inv = window.__TAURI_INTERNALS__.invoke;`

// 2. Turn the endpoint on; the returned state carries the URL and token.
const st = await ev(PORT, `${helpers} return inv('mcp_local_set', { enabled: true })`)
check('mcp_local_set reports running', st.enabled && st.running, st.url)

// 3. A tiny streamable-HTTP JSON-RPC client: session id from initialize,
//    replies arrive as JSON or an SSE stream depending on rmcp's mood.
let nextId = 1
let session = null
let protocolOk = false
async function rpc(method, params, opts = {}) {
  const res = await fetch(st.url, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      Accept: 'application/json, text/event-stream',
      ...(opts.noAuth ? {} : { Authorization: `Bearer ${st.token}` }),
      ...(session ? { 'Mcp-Session-Id': session } : {}),
    },
    body: JSON.stringify(
      opts.notification ? { jsonrpc: '2.0', method, params } : { jsonrpc: '2.0', id: nextId++, method, params }
    ),
  })
  if (opts.expectStatus) return res.status
  if (!res.ok) throw new Error(`${method}: HTTP ${res.status}`)
  const sid = res.headers.get('mcp-session-id')
  if (sid) session = sid
  if (opts.notification) return null
  const ct = res.headers.get('content-type') || ''
  let msg
  if (ct.includes('text/event-stream')) {
    const text = await res.text()
    for (const line of text.split('\n')) {
      if (!line.startsWith('data:')) continue
      try {
        const j = JSON.parse(line.slice(5).trim())
        if (j.id !== undefined) { msg = j; break }
      } catch { /* keep-alive or comment line */ }
    }
  } else {
    msg = await res.json()
  }
  if (!msg) throw new Error(`${method}: no reply in response`)
  if (msg.error) throw new Error(`${method}: ${JSON.stringify(msg.error)}`)
  // A tool that failed answers with isError + its message in the content,
  // not as a JSON-RPC error — surface it the same way either way.
  if (msg.result?.isError) {
    throw new Error(
      `${method}: ${(msg.result.content || []).map((c) => c.text).join(' ') || 'tool error'}`
    )
  }
  return msg.result
}

let version = '2025-06-18'
let init
for (;;) {
  try {
    init = await rpc('initialize', {
      protocolVersion: version,
      capabilities: {},
      clientInfo: { name: 'kawruh-e2e', version: '0.0.0' },
    })
    break
  } catch (e) {
    if (version === '2025-06-18') { version = '2025-03-26'; continue }
    throw e
  }
}
protocolOk = !!init?.serverInfo?.name
check('initialize answers with a server', protocolOk, `${init?.serverInfo?.name} ${init?.serverInfo?.version}`)
await rpc('notifications/initialized', {}, { notification: true })

const tools = await rpc('tools/list', {})
const names = (tools.tools || []).map((t) => t.name)
check(
  'tools/list has the tool set',
  ['list_workspaces', 'list_pages', 'read_page', 'search_pages', 'create_page', 'update_page',
   'rename_page', 'move_page', 'delete_page', 'upload_image', 'list_backlinks', 'list_comments',
   'add_comment', 'query_database'].every((t) => names.includes(t)),
  `${names.length} tools`
)

const text = (r) => (r.content || []).map((c) => c.text).join('\n')

// 4. The workspace shows up.
const listed = text(await rpc('tools/call', { name: 'list_workspaces', arguments: {} }))
check('list_workspaces sees the local workspace', listed.includes(`MCP E2E ${run}`), listed.split('\n')[0] || '')

// 5. Create a page through MCP, then read it back through the app.
const created = text(await rpc('tools/call', {
  name: 'create_page',
  arguments: { workspace_id: ws.id, title: `MCP page ${run}`, markdown: 'Written by the MCP client.' },
}))
const pageId = (created.match(/\(([0-9a-f-]{6,})\)\.?$/) || [])[1]
check('create_page returns a page id', !!pageId, created.trim())
const inApp = await ev(
  PORT,
  `${helpers} return inv('page_export_markdown', { workspaceId: '${ws.id}', pageId: '${pageId}' })`
)
check('the page landed in the app with its body', inApp.includes('Written by the MCP client.'), JSON.stringify(inApp.slice(0, 60)))

// 6. Appends and search agree with the app.
await rpc('tools/call', { name: 'update_page', arguments: { page_id: pageId, markdown: '\nAppended line.', mode: 'append' } })
const after = await ev(
  PORT,
  `${helpers} return inv('page_export_markdown', { workspaceId: '${ws.id}', pageId: '${pageId}' })`
)
check('update_page append lands in the app', after.includes('Appended line.'))
const found = text(await rpc('tools/call', { name: 'search_pages', arguments: { query: 'Appended line' } }))
check('search_pages finds the new text', found.includes(`MCP page ${run}`), found.split('\n')[0] || '')

// 7. Backlinks through the exact kawruh://page/ link.
const other = text(await rpc('tools/call', {
  name: 'create_page',
  arguments: { workspace_id: ws.id, title: `Linker ${run}`, markdown: `See [MCP page ${run}](kawruh://page/${pageId}).` },
}))
const linkerId = (other.match(/\(([0-9a-f-]{6,})\)\.?$/) || [])[1]
const links = text(await rpc('tools/call', { name: 'list_backlinks', arguments: { page_id: pageId } }))
check('list_backlinks finds the linking page', links.includes(`Linker ${run}`) && links.includes(linkerId), links.split('\n')[0] || '')

// 8. Comments round-trip, visible in the app's own format.
const commented = text(await rpc('tools/call', { name: 'add_comment', arguments: { page_id: pageId, body: 'A note from the agent.' } }))
check('add_comment returns a comment id', commented.includes('Comment ID:'))
const comments = text(await rpc('tools/call', { name: 'list_comments', arguments: { page_id: pageId } }))
check('list_comments shows the thread', comments.includes('You: A note from the agent.'), comments.split('\n')[0] || '')

// 9. A database page reads as rows.
const db = text(await rpc('tools/call', {
  name: 'create_page',
  arguments: {
    workspace_id: ws.id,
    title: `DB ${run}`,
    markdown: '```kawruh-database\n{"columns":[{"id":"c1","name":"Name","type":"text"},{"id":"c2","name":"Qty","type":"number"}],"rows":[{"id":"r1","cells":{"c1":"Tea","c2":3}},{"id":"r2","cells":{"c1":"Coffee","c2":7}}]}\n```',
  },
}))
check('create_page makes a database page', db.includes('Created page'))
const dbId = (db.match(/\(([0-9a-f-]{6,})\)\.?$/) || [])[1]
const rows = text(await rpc('tools/call', {
  name: 'query_database',
  arguments: { page_id: dbId, column: 'Name', value: 'Coffee' },
}))
check('query_database filters rows', rows.includes('Coffee') && !rows.includes('Tea'), rows.split('\n')[0] || '')

// 10. The desktop refuses page sharing, politely.
let shareErr = ''
try { await rpc('tools/call', { name: 'share_page', arguments: { page_id: pageId } }) } catch (e) { shareErr = String(e) }
check('share_page is refused on the desktop', shareErr.includes('no public page sharing'), shareErr.slice(0, 80))

// 11. A bad token gets nothing.
const bad = await rpc('tools/list', {}, { noAuth: true, expectStatus: true })
check('requests without the token are rejected', bad === 401, `HTTP ${bad}`)

// 12. Off again: the endpoint closes, the workspace stays.
const off = await ev(PORT, `${helpers} return inv('mcp_local_set', { enabled: false })`)
check('mcp_local_set stops the endpoint', !off.enabled && !off.running)
let closed = false
try { await fetch(st.url, { method: 'POST', body: '{}' }); } catch { closed = true }
check('the port is closed after the toggle', closed)

await ev(PORT, `${helpers} return inv('workspace_forget', { workspaceId: '${ws.id}' })`)

console.log(failures === 0 ? `\nALL PASS (${process.argv[2] ?? ''})` : `\n${failures} FAIL`)
process.exit(failures === 0 ? 0 : 1)
