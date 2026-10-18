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
| DELETE | `/api/workspaces/:id` | — | owner | `204` |
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

`url` = `KAWRUH_PUBLIC_URL + /s/<token>`.

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
