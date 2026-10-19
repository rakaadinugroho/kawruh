# Kawruh server API (v1)

Base URL: `http://<host>:7420`. JSON in/out. Authenticated endpoints take
`Authorization: Bearer <token>` (a session token from login/register, or a
personal access token `kwr_…`). Errors: HTTP status + `{"error": "<message>", "code": "<snake_case>"}`.

## Auth

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/auth/register` | `{username, password, display_name?}` | `200 {token, user}` · `409 username_taken` · `403 signup_disabled` · `400 invalid` |
| POST | `/api/auth/login` | `{username, password}` | `200 {token, user}` · `401 invalid_credentials` |
| POST | `/api/auth/logout` | — | `204` (revokes the presented session token) |
| GET | `/api/me` | — | `200 user` |

`user` = `{id, username, display_name}`. Username: 3–32 chars `[a-z0-9_.-]` (lower-cased);
password ≥ 8 chars. Passwords hashed with argon2id. Tokens are random 32 bytes,
stored as SHA-256 hashes.

## Personal access tokens (for MCP)

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/tokens` | — | `200 [{id, name, created_at, last_used_at}]` |
| POST | `/api/tokens` | `{name}` | `200 {id, name, token}` — `token` (`kwr_…`) shown once |
| DELETE | `/api/tokens/:id` | — | `204` |

## Workspaces & members

Roles: `owner` > `editor` > `viewer`.

| Method | Path | Body | Who | Response |
|---|---|---|---|---|
| GET | `/api/workspaces` | — | any | `200 [{id, name, role}]` |
| POST | `/api/workspaces` | `{id?, name}` | any | `200 {id, name, role}`; re-posting an id you own is idempotent (used to publish a local workspace); someone else's id → `409 workspace_exists` |
| DELETE | `/api/workspaces/:id` | — | owner | `204` (documents and assets included) |
| GET | `/api/workspaces/:id/members` | — | member | `200 [{user_id, username, display_name, role}]` |
| POST | `/api/workspaces/:id/members` | `{username, role: "editor"\|"viewer"}` | owner | `200 member` (upsert) · `404 user_not_found` |
| DELETE | `/api/workspaces/:id/members/:user_id` | — | owner | `204` (owner cannot be removed) |

## Share links (public, read-only)

| Method | Path | Body | Who | Response |
|---|---|---|---|---|
| GET | `/api/workspaces/:id/shares` | — | member | `200 [{token, page_id, url, created_at}]` |
| POST | `/api/workspaces/:id/shares` | `{page_id}` | editor+ | `200 {token, page_id, url}` (returns the existing link if one exists for that page) |
| DELETE | `/api/shares/:token` | — | editor+ | `204` |
| GET | `/s/:token` | — | public | HTML of the shared page (Mermaid rendered client-side, canvas as stored SVG) |
| GET | `/s/:token/:page_id` | — | public | HTML of a descendant page of the shared page |
| GET | `/s/:token/assets/:name` | — | public | An asset of the shared page's workspace (see Assets) · `404` otherwise |

`url` = `KAWRUH_PUBLIC_URL + /s/<token>`. A revoked link (or a shared page that was
deleted) also stops serving its assets.

Share pages render a paragraph that holds only a link titled `kawruh:card` or
`kawruh:preview` as a link card:

```markdown
[Example Domain](https://example.com "kawruh:card")

[Example Domain](https://example.com "kawruh:preview")
```

(Each card is its own paragraph; a card link with other text around it stays a plain link.)

`kawruh:card` puts the image to the right of the text, `kawruh:preview` puts a large image
on top. Card metadata lives in the page doc's root `Y.Map` named `link_meta`:
key = the link's href, value = a JSON string
`{"title", "description", "image", "icon", "site"}` (all optional). The title falls back to
the link text, the site to the URL's domain; without metadata a card shows title + domain.
Only `http(s)` URLs are used for the card link, image and icon; a card link with any other
scheme is rendered as plain text. Share pages allow images from `'self'`, `data:`, `https:`
and `http:` (CSP `img-src`).

## Assets (images)

Content-addressed images, stored at `<KAWRUH_DATA>/assets/<workspace_id>/<name>`.
Name: `<sha256 of the bytes, 64 lowercase hex>.<ext>` with ext `png` · `jpg` · `gif` · `webp` ·
`avif` (served as `image/png`, `image/jpeg`, `image/gif`, `image/webp`, `image/avif`); any other
name → `400 bad_name`. Page markdown refers to an asset as
`kawruh-asset://<workspace_id>/<name>`, e.g. `![](kawruh-asset://<workspace_id>/3f…9c.png)`;
share pages rewrite those of their own workspace to `/s/<token>/assets/<name>` (references to
other workspaces are left as they are and do not load).

| Method | Path | Body | Who | Response |
|---|---|---|---|---|
| PUT | `/api/workspaces/:id/assets/:name` | raw bytes | editor+ | `201 {name, size}` new · `200 {name, size}` already stored (idempotent) · `400 bad_hash` (sha256 of the body ≠ name) · `400 bad_name` · `413 too_large` (> `KAWRUH_MAX_ASSET_MB`) · `507 disk_full` (free disk < `KAWRUH_MIN_FREE_MB`) |
| GET | `/api/workspaces/:id/assets/:name` | — | member | `200` bytes · `404 not_found` · `400 bad_name` |

Both take the usual bearer token (session or PAT); non-members get `403 forbidden`, an unknown
workspace `404 workspace_not_found`. Responses carry `X-Content-Type-Options: nosniff` and
`Cache-Control: private, max-age=31536000, immutable` (`public, …` on the share route).

## Health

`GET /api/health` → `{ok: true, version, disk_free_bytes}`.

## Sync socket

`GET /api/ws?workspace=<id>` with `Authorization: Bearer <token>` (or `&token=<token>`).
`401` bad token, `403` not a member. Binary frames `[kind u8][doc_id_len u8][doc_id][payload]`
(see `kawruh_core::protocol`):

- `SUBSCRIBE` (2): start syncing `doc_id`. The workspace doc's id equals the workspace id.
  An unknown doc id is registered to this workspace when the subscriber is an editor;
  a doc id that belongs to another workspace gets `ERROR forbidden`.
  On subscribe the server sends its own step1 and the current awareness states.
- `SYNC` (0): y-sync message. step1 → server answers step2. step2/update from a
  viewer → dropped + one `ERROR readonly`; otherwise applied and broadcast to other subscribers.
- `AWARENESS` (1): broadcast to other subscribers; the server remembers each
  connection's awareness client ids and broadcasts their removal on disconnect.
- `UNSUBSCRIBE` (3), `ERROR` (4, JSON `{code, message}`).

## MCP

Streamable HTTP at `/mcp`, `Authorization: Bearer <token>`. Tools (writes need editor role;
edits are applied to the live documents and reach open editors immediately):

| Tool | Arguments |
|---|---|
| `list_workspaces` | — |
| `list_pages` | `workspace_id` |
| `read_page` | `page_id` |
| `search_pages` | `query`, `workspace_id?` |
| `create_page` | `workspace_id`, `title`, `markdown?`, `parent_id?` |
| `update_page` | `page_id`, `markdown`, `mode?: "replace" \| "append"` (default replace) |
| `rename_page` | `page_id`, `title` |
| `move_page` | `page_id`, `parent_id?` (null/absent = root), `index?` |
| `delete_page` | `page_id` (soft delete; restorable in the app's Trash) |

## Server configuration (env)

| Var | Default |
|---|---|
| `KAWRUH_BIND` | `0.0.0.0:7420` |
| `KAWRUH_DATA` | `./data` (SQLite at `<data>/kawruh.db`) |
| `KAWRUH_PUBLIC_URL` | `http://localhost:7420` |
| `KAWRUH_ALLOW_SIGNUP` | `true` |
| `KAWRUH_MIN_FREE_MB` | `50` — below this, document writes are refused |
| `KAWRUH_MAX_ASSET_MB` | `10` — largest accepted asset upload |
