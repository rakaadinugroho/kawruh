# Use Kawruh with AI agents (MCP)

Every Kawruh server has an [MCP](https://modelcontextprotocol.io) endpoint at
`/mcp` (streamable HTTP). Connect an agent such as Claude Code, Claude Desktop,
Cursor or VS Code to it, and the agent can list, read, search, write and
organise your pages.

The agent acts as you. It sees the workspaces you are a member of, with your
role in each. Its edits land in the live documents, so anyone with the page
open sees them as they happen.

The examples below use `https://kawruh.example.com`. Replace it with your
server's address.

## Before you start

- **A Kawruh server you can reach.** Use `https://…` for a server behind HTTPS
  (for example a Cloudflare Tunnel), or `http://<host>:7420` on your own network.
- **An account on that server.** Create it in the app: Settings → Accounts.
- **Published workspaces.** Agents only see workspaces on the server. A workspace
  that lives only on your computer ("On this Mac only") is invisible to them
  until you publish it with **Share → Publish**.

## 1. Create a token

In the app, go to **Settings → API tokens → New token**. Copy the `kwr_…` value
right away; it is shown only once. The app also shows a ready-to-paste Claude
Code command.

Make one token per agent or computer, so you can revoke each one on its own.

<details>
<summary>Without the app (curl)</summary>

```sh
SERVER=https://kawruh.example.com
SESSION=$(curl -s $SERVER/api/auth/login -H 'Content-Type: application/json' \
  -d '{"username":"you","password":"your-password"}' | jq -r .token)
curl -s $SERVER/api/tokens -H "Authorization: Bearer $SESSION" \
  -H 'Content-Type: application/json' -d '{"name":"claude-code"}' | jq -r .token
```

</details>

## 2. Connect your agent

All clients connect the same way: they send the token as an
`Authorization: Bearer kwr_…` header to `<server>/mcp`.

### Claude Code

```sh
claude mcp add --transport http kawruh https://kawruh.example.com/mcp \
  --header "Authorization: Bearer kwr_..."
```

Add `--scope user` to use it in every project, not just the current one.

### Claude Desktop

Claude Desktop reaches remote servers with a custom header through
[`mcp-remote`](https://www.npmjs.com/package/mcp-remote), which needs Node.js.
Open **Settings → Developer → Edit Config** (`claude_desktop_config.json`), add
the following, and restart Claude Desktop:

```json
{
  "mcpServers": {
    "kawruh": {
      "command": "npx",
      "args": [
        "mcp-remote",
        "https://kawruh.example.com/mcp",
        "--header",
        "Authorization:${KAWRUH_AUTH}"
      ],
      "env": { "KAWRUH_AUTH": "Bearer kwr_..." }
    }
  }
}
```

Keep `Authorization:${KAWRUH_AUTH}` without spaces. Some clients mangle spaces
inside `args`, and spaces inside `env` are fine.

### Cursor

Add this to `~/.cursor/mcp.json` (for all projects) or `.cursor/mcp.json` (for
one project):

```json
{
  "mcpServers": {
    "kawruh": {
      "url": "https://kawruh.example.com/mcp",
      "headers": { "Authorization": "Bearer ${env:KAWRUH_TOKEN}" }
    }
  }
}
```

Then set `KAWRUH_TOKEN=kwr_...` in the environment Cursor starts from, or put
the token in the header directly.

### VS Code (Copilot agent mode)

Add this to `.vscode/mcp.json`. VS Code asks for the token the first time and
stores it securely:

```json
{
  "inputs": [
    { "type": "promptString", "id": "kawruh-token", "description": "Kawruh token (kwr_…)", "password": true }
  ],
  "servers": {
    "kawruh": {
      "type": "http",
      "url": "https://kawruh.example.com/mcp",
      "headers": { "Authorization": "Bearer ${input:kawruh-token}" }
    }
  }
}
```

### Other clients

Any MCP client that supports streamable HTTP with a custom header works the same
way: use the URL `<server>/mcp` and the header `Authorization: Bearer kwr_…`.

## 3. Check it works

- **Claude Code:** `claude mcp list` should show `kawruh` as connected. Inside a
  session, `/mcp` lists its tools.
- **Any client:** ask *"List my Kawruh workspaces."* The agent should name them
  and your role in each.

## How agents work with your pages

You don't need to know any ids; ask in plain language. Behind the scenes an
agent usually:

1. calls `list_workspaces` to find the workspace,
2. calls `list_pages` to get the page tree with ids, or `search_pages` to find
   pages by content,
3. calls `read_page` to read a page as markdown,
4. calls `create_page`, `update_page`, `rename_page`, `move_page` or
   `delete_page` to make changes.

Pages an agent deletes go to the Trash, and you can restore them in the app.

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
| `upload_image` | `workspace_id`, `data_base64`, `mime` (png \| jpeg \| gif \| webp \| avif), `alt?`, `page_id?` | Store an image; returns its markdown `![alt](kawruh-asset://…)`, and appends it to `page_id` if given |
| `list_backlinks` | `page_id` | Pages in the same workspace that link to this page, as `- Title (page_id)` |
| `share_page` | `page_id` | Create (or return) the page's public read-only link; returns the URL |
| `list_comments` | `page_id` | The page's comments as threads, each with its author and the text it is about |
| `add_comment` | `page_id`, `body`, `reply_to?` | Write a comment, or a reply in an existing thread (editor+) |
| `query_database` | `page_id`, `column?`, `value?`, `limit?` | The rows of a database page as a markdown table; with `column` only the rows whose cell holds `value` are returned |

Write tools need the **editor** or **owner** role; viewers get a read-only error.

## Writing pages

Page bodies are GitHub-flavoured markdown. Titles are not part of the body; use
`rename_page` to change them. A few conventions:

- **Diagrams:** ` ```mermaid ` fenced blocks render as Mermaid in the app and on
  share pages.
- **Drawings:** ` ```kawruh-canvas ` blocks hold a canvas id. Leave them in place
  when rewriting a page, or the drawing is detached from the page.
- **Link cards:** a paragraph that is only `[Title](https://… "kawruh:card")`
  shows as a bookmark card (`"kawruh:preview"` for a large preview). Elsewhere it
  reads as a normal link, so it's safe to write.
- **Page links:** link to another page with `[Title](kawruh://page/<page_id>)`.
  Use `list_pages` or `search_pages` to find the id. On a share page, links to
  pages inside the shared subtree keep working; other page links show as plain text.
- **Images:** images pasted in the app appear as
  `![](kawruh-asset://<workspace>/<sha256>.png)`. Keep those refs when rewriting
  a page. Agents add new images with `upload_image`, which returns that markdown;
  ordinary `https://` image URLs work too.
- **Comments:** `add_comment` writes into the page document, so it appears
  live in open editors. Mention someone with `@[Name](kawruh://user/<user_id>)`
  (the id is their user id) and they get a notification; so does everyone else in
  a thread you reply to. `list_comments` shows what is already there, including
  the replies. Comments are not part of the markdown — `read_page` won't show
  them.
- **Search** reads an index refreshed about two seconds after each edit.

## Databases

A page can be a **database**: a table of rows with typed properties (name,
text, number, status, date, checkbox, person, link). The app opens it as a
table or as a board. Rows are not pages, so they never show up in the page
tree.

An agent makes a page a database by writing a `kawruh-database` block into its
body, with `create_page` or `update_page`:

````markdown
```kawruh-database
{"columns":[{"id":"name","name":"Name","type":"title"},{"id":"status","name":"Status","type":"select","options":["Todo","Doing"]}],"rows":[{"id":"r1","cells":{"name":"Write the docs","status":"Doing"}}]}
```
````

The columns are `{id, name, type, options?}` — `type` is one of `title`,
`text`, `number`, `select`, `multi_select`, `date`, `checkbox`, `person`,
`url`, and `options` lists the choices of a `select` column. The rows are
`{id, cells}` and every cell is written under the id of its column.

Afterwards `read_page` shows the rows as a markdown table, and
`query_database` reads them:

```json
{"page_id": "…", "column": "Status", "value": "Doing"}
```

With `column` and `value`, only the rows whose cell holds that value come
back; `limit` caps how many rows are returned (50 by default).

Keep the block when you rewrite the page's body — without it the page is an
ordinary page again.

## Example prompts

- "List my Kawruh workspaces and summarise the pages under *Projects*."
- "Search my notes for *pricing* and give me the three main decisions."
- "Read *Roadmap* and append a *Risks* section with what you think could slip."
- "Create meeting notes under *Meetings* from this transcript, with action items
  as a checklist."
- "Turn this repository's README into a page under *Docs*."
- "Add a Mermaid sequence diagram of the login flow to the *Auth* page."
- "Move every page with *draft* in its title under *Archive*."
- "Read the comments on *Roadmap* and post a summary for the team."
- "Comment on *Pricing* asking @Ana whether the numbers include tax."
- "Make a *Tasks* database with the columns Name, Status and Due, and add the
  action items from this transcript as rows."
- "Show me the rows of the *Tasks* database whose Status is *Doing*."

## Access and safety

- **A token can do whatever your account can.** In workspaces where you are a
  viewer, the agent can only read.
- **For a read-only agent,** create a separate account, invite it to the
  workspace as a **viewer**, and make the token on that account.
- **Revoke** a token in Settings → API tokens (or `DELETE /api/tokens/<id>`). It
  stops working immediately. The server only stores a hash of each token.
- **Use HTTPS** whenever the server is reached over the internet. Plain HTTP is
  only for a network you trust.

## Troubleshooting

| What you see | Likely cause |
|---|---|
| `401 unauthorized` | The token was mistyped or revoked, or the header isn't exactly `Authorization: Bearer kwr_…`. |
| "read-only" error on writes | You're a viewer in that workspace. |
| The agent can't find a workspace or page | The workspace isn't published to this server, or recent edits haven't synced yet. Wait for **Synced** in the app's sidebar. |
| A brand-new page doesn't show up in search | The search index catches up about two seconds after an edit. |
| Connection or TLS errors | Check the address: `https://` for a server behind HTTPS, `http://…:7420` on your own network. |
