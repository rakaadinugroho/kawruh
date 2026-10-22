# Kawruh

A markdown notebook for the desktop. Pages nest like Notion, the editor is
WYSIWYG but everything is markdown underneath, and Mermaid diagrams, canvas
drawings, pasted screenshots and link cards live right in the page. Workspaces
stay on your computer until you publish them to your own Kawruh server; then
people edit together live, read-only links can be shared, and AI agents can read
and write pages over MCP.

- **Link pages** with `[[`, and see what links back to each page.
- **Graph view** (⌘⇧G) draws your pages as dots and the links between them as lines.
- **Comments** on a page or on a piece of text (⌘⇧M), with `@mentions`; mentions
  and replies land in your **inbox**.
- **Databases**: turn a page into a table with typed columns, or a board you can
  drag cards across.
- **Search** titles and page text with ⌘K.
- **Templates** (daily note, meeting notes, your own) and **page history** with restore.
- **Import** a Notion export or an Obsidian vault; **export** a workspace, a page
  (with its sub-pages) as markdown, or as PDF.
- **Open `.md` files** with Kawruh, pick a serif, sans or mono font, and get
  **updates** automatically.

*Kawruh* is Javanese for knowledge.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screenshots/hero-dark.png">
  <img alt="Kawruh with a nested page tree in the sidebar and a page showing a Mermaid diagram and a checklist" src="docs/screenshots/hero-light.png">
</picture>

| Canvas drawings sit inside the page | ⌘K jumps to any page |
|---|---|
| <img alt="A page with an embedded hand-drawn canvas sketch" src="docs/screenshots/canvas.png"> | <img alt="The quick switcher filtering pages by title" src="docs/screenshots/switcher.png"> |

## Download

Get the latest `.dmg` (macOS 11+, Apple Silicon and Intel) or `setup.exe`
(Windows 10/11) from [Releases](https://github.com/rakaadinugroho/kawruh/releases/latest).
The builds aren't signed by Apple or Microsoft yet, so the first launch takes one
extra click: on macOS, System Settings → Privacy & Security → **Open Anyway**; on
Windows, **More info → Run anyway**.

## Use it with AI agents (MCP)

Every Kawruh server has an MCP endpoint at `/mcp`, so Claude Code, Claude
Desktop, Cursor, VS Code and other agents can read, search and edit your pages.
Their edits show up live for everyone with the page open.

1. In the app, open **Settings → API tokens → New token** and copy the `kwr_…` token.
2. Connect your agent, for example Claude Code:
   ```sh
   claude mcp add --transport http kawruh https://<your-server>/mcp \
     --header "Authorization: Bearer kwr_..."
   ```
3. Ask it something like *"List my Kawruh workspaces"* or *"Summarise the pages under Projects"*.

Agents see only workspaces published to the server. Setup for the other clients,
the full tool list, and tips are in [docs/MCP.md](docs/MCP.md).

## How it fits together

```
Kawruh.app (Tauri 2)                         kawruh-server (homelab, :7420)
 React + Milkdown Crepe  ◄─IPC─►  Rust DocHub ──WS──►  sync · auth · shares · /mcp
 (mirror Y.Docs)                  (yrs + SQLite)        (yrs + SQLite)
```

- **Rust owns every document.** The webview edits mirror Yjs docs; Rust persists
  them in SQLite and syncs them. You can work offline; edits merge on reconnect.
- **Markdown is the contract.** `kawruh-core` converts markdown ↔ the editor's
  Yjs structure in Rust, checked against the editor's real schema
  (`crates/kawruh-core/schema.json`, `scripts/contract.sh`).
- Details: [docs/PLAN.md](docs/PLAN.md) (design), [docs/API.md](docs/API.md)
  (server API and sync protocol), [docs/MCP.md](docs/MCP.md) (agents).

```
crates/kawruh-core     shared: markdown↔Yjs, page tree, SQLite store, sync protocol + client
crates/kawruh-server   axum server: auth, workspaces, WS sync, share pages, MCP
app/                   desktop app: React UI (src/) + Tauri/Rust (src-tauri/)
deploy/                systemd unit, deploy and admin scripts for the homelab
```

## Develop

Requires Rust (stable), Node 22+ and pnpm. For deploying the server you also need
`zig` and `cargo-zigbuild` (`brew install zig && cargo install cargo-zigbuild`).

```sh
cd app && pnpm install
pnpm tauri dev                 # desktop app with hot reload
```

`app/.env.local` can set `VITE_DEFAULT_SERVER=http://<host>:7420` to prefill the
server field. `KAWRUH_DATA_DIR` points the app at another data folder (handy for
running two instances side by side).

Run a server locally:

```sh
KAWRUH_DATA=./data cargo run -p kawruh-server     # http://localhost:7420
```

## Build the app

```sh
cd app && pnpm tauri build                 # target/release/bundle/macos/Kawruh.app + .dmg
pnpm tauri build --bundles app             # just the .app
```

Local builds are ad-hoc signed; macOS opens them without complaint on the Mac that built them.

## Release

Pushing a version tag builds the macOS (universal `.dmg`) and Windows (`setup.exe`)
apps plus the server binaries on GitHub Actions and publishes them as a release:

```sh
scripts/release.sh 0.2.0 && git push --atomic origin main v0.2.0
```

Details, signing secrets and deploying a release build: [docs/RELEASE.md](docs/RELEASE.md).

## Server on the homelab

```sh
KAWRUH_SSH_PASS=… deploy/deploy.sh          # cross-compile (aarch64-musl), upload, restart
KAWRUH_SSH_PASS=… deploy/deploy.sh --release v0.2.0   # install a release build instead
KAWRUH_SSH_PASS=… deploy/deploy.sh --rollback
KAWRUH_SSH_PASS=… deploy/admin.sh list-users
KAWRUH_SSH_PASS=… deploy/admin.sh invite                # prints a sign-up code
KAWRUH_SSH_PASS=… deploy/admin.sh delete-user <name>   # also removes workspaces they own
KAWRUH_SSH_PASS=… deploy/admin.sh gc --dry-run         # what cleanup would remove
KAWRUH_SSH_PASS=… deploy/admin.sh gc                    # frees disk now
curl http://<host>:7420/api/health
```

The scripts deploy to `KAWRUH_HOST` (`user@host`); put it in `deploy/.env.local`
(gitignored) so you don't have to type it.

`gc` is the disk cleanup: it deletes images no page refers to any more and the
documents of pages purged from the trash. An image is kept for a week after it
stops being used, so a page can still be restored in the meantime. The server
does the same on its own, 10 minutes after start and then once a day.

Config lives in `/etc/kawruh/kawruh.env`, data in `/var/lib/kawruh`. For a server
others can reach:

- `KAWRUH_ALLOW_SIGNUP=false` makes it invite-only. The first account can always be
  created; after that, people need a code from **Settings → Accounts → Invite**
  (or `admin invite`). Codes work once, for 7 days.
- `KAWRUH_QUOTA_MB=200` caps the images each workspace owner can store (default: no cap).
- Logins are limited to 10 failures per 15 minutes per username or address.

## Tests

```sh
cargo test --workspace          # converter round-trips, store, protocol, server integration
scripts/contract.sh             # Rust converter ↔ editor schema, both directions
cd app && pnpm test             # JS side of the contract
node app/tests/e2e/collab.mjs --server http://<host>:7420   # two live app instances, see the file header
```

Debug builds started with `KAWRUH_E2E_PORT` expose a localhost hook used by the
E2E script (`app/tests/e2e/ev.sh` evaluates JS, `snap.sh` snapshots the window).
It is compiled out of release builds.

## Known limits (v1)

- Tested on macOS. Windows builds and passes tests in CI but hasn't been tried on a
  real machine; its title bar keeps the macOS traffic-light gutter.
- Permissions are per workspace (owner / editor / viewer), not per page.
- Images up to 10 MB each (PNG, JPEG, GIF, WebP, AVIF). The server deletes the
  ones no page uses any more, but only after a week.
- The server speaks plain HTTP: use it on your LAN or over Tailscale. Exposing it
  publicly needs HTTPS in front (e.g. a Cloudflare tunnel).
- Server tokens are stored unencrypted in the app's local database.

## License

MIT, see [LICENSE](LICENSE).
