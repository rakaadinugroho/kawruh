# Connecting AI agents to Kawruh (MCP)

The Kawruh server exposes an MCP endpoint at `http://<host>:7420/mcp`
(streamable HTTP). Agents act as you: they see the workspaces you are a member
of, and their edits land in the live documents, so anyone with the page open
sees the change immediately.

## 1. Create a personal access token

**In the app:** Settings → API tokens → *New token*. Copy the `kwr_…` value; it
is shown once.

**With curl** (using your username and password):

```sh
SERVER=http://<host>:7420
SESSION=$(curl -s $SERVER/api/auth/login -H 'Content-Type: application/json' \
  -d '{"username":"you","password":"your-password"}' | jq -r .token)
curl -s $SERVER/api/tokens -H "Authorization: Bearer $SESSION" \
  -H 'Content-Type: application/json' -d '{"name":"claude-code"}' | jq -r .token
```

Revoke a token with `DELETE /api/tokens/<id>` (ids from `GET /api/tokens`) or in
the app.

## 2. Add the server to Claude Code

```sh
claude mcp add --transport http kawruh http://<host>:7420/mcp \
  --header "Authorization: Bearer kwr_..."
```

Check it with `claude mcp list`, or `/mcp` inside a session. Any MCP client that
supports streamable HTTP with a custom `Authorization` header works the same way.

Off your LAN, reach the server through Tailscale (the homelab is on your
tailnet) and use its Tailscale address instead of its LAN address. Traffic is
plain HTTP, so don't expose port 7420 to the internet directly.

## Tools

| Tool | Arguments | What it does |
|---|---|---|
| `list_workspaces` | — | Your workspaces and your role in each |
| `list_pages` | `workspace_id` | Page tree as an outline: `- Title (page_id)` |
| `read_page` | `page_id` | Title, path and the body as markdown |
| `search_pages` | `query`, `workspace_id?` | Case-insensitive search over titles and bodies |
| `create_page` | `workspace_id`, `title`, `markdown?`, `parent_id?` | New page, optionally nested; returns its id |
| `update_page` | `page_id`, `markdown`, `mode?` (`replace` \| `append`) | Rewrite or extend a page body |
| `rename_page` | `page_id`, `title` | Change a page title |
| `move_page` | `page_id`, `parent_id?`, `index?` | Re-parent (null = top level) and/or reorder |
| `delete_page` | `page_id` | Move to Trash (restorable in the app) |

Write tools need the **editor** or **owner** role; viewers get a read-only error.

Page bodies are GitHub-flavoured markdown. Titles are not part of the body: use
`rename_page` to change them. A few conventions:

- Diagrams: ` ```mermaid ` fenced blocks render as Mermaid in the app and on share pages.
- Drawings: ` ```kawruh-canvas ` blocks hold a canvas id; leave them in place when
  rewriting a page or the drawing is detached from the page.
- Link cards: a paragraph that is only `[Title](https://… "kawruh:card")` shows as a
  bookmark card (`"kawruh:preview"` for a large preview). Elsewhere it reads as a
  normal link, so it's safe to write.
- Images pasted in the app appear as `![](kawruh-asset://<workspace>/<sha256>.png)`.
  Keep those refs when rewriting a page; agents can't upload new images yet, but
  ordinary `https://` image URLs work.
- `search_pages` reads an index refreshed about two seconds after each edit.

## Example prompts

- "List my Kawruh workspaces and summarise the pages under *Projects*."
- "Create a page *Release checklist* under *Projects* with a task list for v1."
- "Append a Mermaid sequence diagram of the login flow to the *Auth* page."
