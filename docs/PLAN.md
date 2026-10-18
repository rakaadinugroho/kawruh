# Kawruh — Plan v1

Desktop Markdown notebook (Tauri 2 + Rust), Notion-style page tree, real-time collaboration
via a Rust sync server on the homelab, Mermaid + canvas blocks, MCP-ready.

## Building

A local-first desktop app. Every page is a Yjs CRDT document owned by the Rust side of the app
(stored in local SQLite), rendered/edited by a Milkdown Crepe WYSIWYG editor in the webview.
Workspaces are either **local** (this machine only) or **remote** (synced with `kawruh-server`
on the homelab). Remote workspaces support members (owner/editor/viewer), live co-editing with
cursors, public read-only share links, and an MCP endpoint so AI agents read/write pages as
markdown — their edits appear live in every open editor.

## Not building (v1)

- Windows build/testing (code stays cross-platform, no macOS-only APIs)
- Page-level permissions (workspace-level roles only)
- Image/file uploads (images by URL only)
- Public internet exposure / HTTPS (LAN + Tailscale; Cloudflare tunnel needs the owner's domain)
- Code signing, notarization, auto-update
- Version history UI, comments
- Local MCP endpoint in the desktop app (MCP lives on the server; core is shared so it's a thin add later)
- Full-text search inside the app UI (title quick-switcher in app; full-text via MCP `search_pages`)

## Key decisions

1. **"Rust pure" = all logic in Rust; UI in TypeScript.** Storage, CRDT hub, sync, markdown
   conversion, server, MCP: Rust. The webview UI is React + TS because the editor (ProseMirror),
   Mermaid and Excalidraw only exist in JS; a Rust-WASM UI would still have to wrap them.
2. **Rust is the document hub.** The webview never talks to the network. Its Y.Doc syncs with the
   Rust-side `yrs` doc over Tauri IPC; Rust persists to SQLite and syncs with the server. This
   gives offline editing, one local source of truth, and server-side MCP edits flowing to the UI.
3. **Milkdown Crepe** over BlockNote/TipTap: markdown-native schema (lossless for agents),
   built-in slash menu + drag handle + code blocks with preview, Yjs collab plugin.
4. **Mermaid = plain ```` ```mermaid ```` code block** with live preview (lossless markdown, no
   custom node). **Canvas = custom `kawruh_canvas` node** (`id` attr) with Excalidraw scene stored
   per element in a Y.Map inside the same page doc; markdown form ```` ```kawruh-canvas\n<id>\n``` ````.
5. **Server = single static Rust binary** (axum + yrs + SQLite), cross-compiled on the Mac
   (`cargo zigbuild`, aarch64-musl), run by systemd on port 7420. The homelab has 412 MB free disk
   and ~590 MB free RAM, no Rust toolchain → no Docker, no on-server builds.

## Architecture

```
 ┌──────────────── Kawruh.app (macOS) ────────────────┐
 │ Webview (React)                 Rust (Tauri)        │
 │  Crepe editor ─ Y.Doc ◄─IPC─►  DocHub (yrs docs)   │
 │  Tree (ws Y.Doc)  awareness ─►   │   ▲               │
 │                                  ▼   │               │
 │                          SQLite (local)  SyncClient ─┼──WS──┐
 └─────────────────────────────────────────────────────┘      │
                                                               ▼
                     ┌──────── kawruh-server (homelab :7420) ────────┐
 Claude Code ──HTTP──►  /mcp (rmcp) ─► DocRegistry (yrs docs) ◄─ /api/ws
 Browser ─────HTTP──►  /s/:token (HTML)      │                         │
                       /api/* (auth, ws, members, shares, tokens)     │
                                   SQLite /var/lib/kawruh/kawruh.db ◄─┘
```
No cycles: the webview ↔ Rust ↔ server direction carries CRDT updates tagged by origin, so an
update is never echoed back to where it came from.

## Data model

- **Workspace doc** (id = workspace id): `meta: Map{name}`, `pages: Map<pageId, Map{title,
  parent ("" = root), order (f64), icon, created, deleted (0 = live)}>`.
- **Page doc** (id = page id): `prosemirror: XmlFragment` (Milkdown schema),
  `canvas:<id>: Map<elementId, JSON string>`, `canvas_svg: Map<canvasId, svg string>`.
- **Markdown ↔ Yjs** conversion lives in `kawruh-core` (Rust, comrak). A **schema contract**
  (`crates/kawruh-core/schema.json`) is dumped from the real Milkdown editor; Rust tests check
  converter output against it, and a cross-language test runs Rust output through y-prosemirror.

## Wire protocol

`GET /api/ws?workspace=<id>&token=<t>` — one WebSocket per workspace, binary frames
`[kind u8][doc_id_len u8][doc_id utf8][payload]`:
`0 SYNC` (yrs SyncMessage: step1/step2/update) · `1 AWARENESS` (y-protocols awareness update) ·
`2 SUBSCRIBE` · `3 UNSUBSCRIBE` · `4 ERROR` (JSON). Viewers' step2/update frames are dropped.

## REST API (server)

`POST /api/auth/register|login` · `GET /api/me` · `GET|POST /api/tokens`, `DELETE /api/tokens/:id`
· `GET|POST /api/workspaces` · `GET|POST /api/workspaces/:id/members`,
`DELETE /api/workspaces/:id/members/:user_id` · `GET|POST /api/workspaces/:id/shares`,
`DELETE /api/shares/:token` · `GET /s/:token[/:page_id]` · `GET /api/health` · `/mcp`.

## MCP tools (server, Bearer token = session or personal access token)

`list_workspaces`, `list_pages`, `read_page`, `search_pages`, `create_page`, `update_page`
(replace|append), `rename_page`, `move_page`, `delete_page` (soft). Writes require editor role.

## Phases (each independently usable)

1. **Local notebook** — Cargo workspace, `kawruh-core` (store, tree ops, md↔Yjs, schema contract),
   Tauri app with DocHub + IPC provider, sidebar tree (nested pages, drag reorder/reparent,
   rename, trash), Crepe editor, Mermaid preview, canvas block, md import/export, ⌘K switcher.
2. **Server + collaboration** — `kawruh-server` (auth, workspaces, members, WS sync, awareness),
   deployed to homelab via systemd; desktop accounts, remote workspaces, publish local workspace,
   live cursors, offline → resync, viewer read-only.
3. **Sharing + MCP** — share links + HTML view (Mermaid rendered, canvas SVG), personal access
   tokens, `/mcp` tools, `docs/MCP.md` with Claude Code setup.

## Verification

- `cargo test --workspace` — converter round-trips + schema contract, store, server integration
  (two sync clients converge, viewer can't write, share HTML, MCP tool calls broadcast live).
- `pnpm -C app test` — cross-language contract: Rust-produced Yjs → y-prosemirror + Milkdown schema → markdown.
- `pnpm -C app tauri build` → `Kawruh.app` builds.
- macOS E2E via a debug-only automation hook (`KAWRUH_E2E_PORT`, compiled out of release):
  create nested pages, type, Mermaid renders, canvas inserts, restart persists; two app instances
  co-edit through the homelab server; MCP `update_page` shows up live; share link renders.
- `curl http://<host>:7420/api/health` after deploy.

## Risks

- **Fragile assumption:** the Rust md→Yjs converter emits exactly what Milkdown's schema accepts.
  If not, y-prosemirror silently deletes the invalid nodes on clients. Guard: schema contract +
  cross-language tests are a merge gate for every converter change.
- **Homelab disk (412 MB free):** if it fills, SQLite writes fail. Server reports free disk on
  `/api/health` and refuses new writes below 50 MB instead of corrupting state.
- **Plain HTTP on LAN:** passwords/tokens travel unencrypted on the LAN. Use Tailscale off-LAN.

## Rollback

Server: `systemctl disable --now kawruh-server`, restore `/opt/kawruh/kawruh-server.prev`; data in
`/var/lib/kawruh` is untouched by deploys. Desktop: local data in the app data dir, untouched by
reinstall.
