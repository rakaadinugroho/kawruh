//! End-to-end tests: a real server on an ephemeral port, real sync clients
//! (`kawruh_core::sync_client`), HTTP and raw MCP JSON-RPC.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kawruh_core::sync_client::{self, SyncHandle, SyncHost, SyncStatus};
use kawruh_core::yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};
use kawruh_core::yrs::updates::decoder::Decode;
use kawruh_core::yrs::updates::encoder::Encode;
use kawruh_core::yrs::{ClientID, Doc, GetString, Map, ReadTxn, Text, Transact};
use serde_json::{Value, json};
use sha2::Digest;

struct Server {
    base: String,
    dir: Arc<tempfile::TempDir>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

async fn start() -> Server {
    start_in(Arc::new(tempfile::tempdir().unwrap())).await
}

async fn start_in(dir: Arc<tempfile::TempDir>) -> Server {
    start_with(dir, |_| {}).await
}

async fn start_with(
    dir: Arc<tempfile::TempDir>,
    configure: impl FnOnce(&mut kawruh_server::Config),
) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut cfg = kawruh_server::Config {
        bind: addr,
        data_dir: dir.path().to_path_buf(),
        public_url: format!("http://{addr}"),
        allow_signup: true,
        min_free_mb: 0,
        max_asset_mb: 10,
    };
    configure(&mut cfg);
    let task = tokio::spawn(kawruh_server::serve(listener, cfg));
    Server {
        base: format!("http://{addr}"),
        dir,
        task,
    }
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

async fn register(s: &Server, username: &str) -> (String, String) {
    let res: Value = http()
        .post(format!("{}/api/auth/register", s.base))
        .json(&json!({ "username": username, "password": "password123" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        res["token"].as_str().unwrap().to_string(),
        res["user"]["id"].as_str().unwrap().to_string(),
    )
}

async fn post(s: &Server, token: &str, path: &str, body: Value) -> (u16, Value) {
    let res = http()
        .post(format!("{}{path}", s.base))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

async fn get(s: &Server, token: Option<&str>, path: &str) -> (u16, Value) {
    let mut req = http().get(format!("{}{path}", s.base));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let res = req.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

async fn create_workspace(s: &Server, token: &str, name: &str) -> String {
    let (status, ws) = post(s, token, "/api/workspaces", json!({ "name": name })).await;
    assert_eq!(status, 200, "{ws}");
    ws["id"].as_str().unwrap().to_string()
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..100 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[derive(Default)]
struct Host {
    docs: Mutex<HashMap<String, Doc>>,
    status: Mutex<Vec<SyncStatus>>,
    awareness: Mutex<Vec<(String, Vec<u8>)>>,
    errors: Mutex<Vec<(String, String)>>,
}

impl SyncHost for Host {
    fn doc(&self, doc_id: &str) -> Option<Doc> {
        Some(
            self.docs
                .lock()
                .unwrap()
                .entry(doc_id.to_string())
                .or_default()
                .clone(),
        )
    }
    fn on_status(&self, status: SyncStatus) {
        self.status.lock().unwrap().push(status);
    }
    fn on_awareness(&self, doc_id: &str, update: Vec<u8>) {
        self.awareness
            .lock()
            .unwrap()
            .push((doc_id.to_string(), update));
    }
    fn on_error(&self, doc_id: &str, code: &str, _message: &str) {
        self.errors
            .lock()
            .unwrap()
            .push((doc_id.to_string(), code.to_string()));
    }
}

struct Client {
    host: Arc<Host>,
    handle: SyncHandle,
}

impl Client {
    fn connect(s: &Server, token: &str, workspace: &str) -> Client {
        let host = Arc::new(Host::default());
        let handle = sync_client::spawn(
            sync_client::ws_url(&s.base, workspace),
            token.to_string(),
            host.clone(),
        );
        Client { host, handle }
    }

    fn doc(&self, id: &str) -> Doc {
        self.host.doc(id).unwrap()
    }

    fn online(&self) -> bool {
        self.host.status.lock().unwrap().last() == Some(&SyncStatus::Online)
    }

    /// Appends text locally and sends the resulting update.
    fn type_text(&self, doc_id: &str, text: &str) {
        let doc = self.doc(doc_id);
        let sv = doc.transact().state_vector();
        let t = doc.get_or_insert_text("t");
        {
            let mut txn = doc.transact_mut();
            let len = t.len(&txn);
            t.insert(&mut txn, len, text);
        }
        let update = doc.transact().encode_state_as_update_v1(&sv);
        self.handle.send_update(doc_id, update);
    }

    fn text(&self, doc_id: &str) -> String {
        let doc = self.doc(doc_id);
        let t = doc.get_or_insert_text("t");
        t.get_string(&doc.transact())
    }
}

#[tokio::test]
async fn auth_register_login_me() {
    let s = start().await;
    let (token, id) = register(&s, "alice").await;

    let (status, me) = get(&s, Some(&token), "/api/me").await;
    assert_eq!(status, 200);
    assert_eq!(me["id"], id.as_str());
    assert_eq!(me["username"], "alice");

    let (status, body) = get(&s, None, "/api/me").await;
    assert_eq!(status, 401);
    assert_eq!(body["code"], "unauthorized");

    let (status, body) = post(
        &s,
        "",
        "/api/auth/register",
        json!({"username": "alice", "password": "password123"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("username_taken"))
    );
    let (status, _) = post(
        &s,
        "",
        "/api/auth/register",
        json!({"username": "bob", "password": "short"}),
    )
    .await;
    assert_eq!(status, 400);

    let (status, login) = post(
        &s,
        "",
        "/api/auth/login",
        json!({"username": "Alice", "password": "password123"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(login["user"]["username"], "alice");
    let (status, body) = post(
        &s,
        "",
        "/api/auth/login",
        json!({"username": "alice", "password": "nope-nope"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (401, Some("invalid_credentials"))
    );

    // Logout revokes the session token.
    let session = login["token"].as_str().unwrap();
    let res = http()
        .post(format!("{}/api/auth/logout", s.base))
        .bearer_auth(session)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 204);
    assert_eq!(get(&s, Some(session), "/api/me").await.0, 401);

    // Personal access tokens authenticate too and are listed without secrets.
    let (status, pat) = post(&s, &token, "/api/tokens", json!({"name": "claude"})).await;
    assert_eq!(status, 200);
    let pat_token = pat["token"].as_str().unwrap();
    assert!(pat_token.starts_with("kwr_"));
    assert_eq!(get(&s, Some(pat_token), "/api/me").await.0, 200);
    let (_, list) = get(&s, Some(&token), "/api/tokens").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert!(list[0].get("token").is_none());

    let (_, health) = get(&s, None, "/api/health").await;
    assert_eq!(health["ok"], true);
    assert!(health["disk_free_bytes"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn workspaces_and_members() {
    let s = start().await;
    let (alice, _) = register(&s, "alice").await;
    let (bob, bob_id) = register(&s, "bob").await;

    let ws = create_workspace(&s, &alice, "Notes").await;
    let (_, list) = get(&s, Some(&alice), "/api/workspaces").await;
    assert_eq!(list[0]["id"], ws.as_str());
    assert_eq!(list[0]["role"], "owner");

    // Publishing with an explicit id is idempotent for the owner, a conflict for others.
    let (status, again) = post(
        &s,
        &alice,
        "/api/workspaces",
        json!({"id": "local-ws-1", "name": "Local"}),
    )
    .await;
    assert_eq!(status, 200, "{again}");
    let (status, _) = post(
        &s,
        &alice,
        "/api/workspaces",
        json!({"id": "local-ws-1", "name": "Local"}),
    )
    .await;
    assert_eq!(status, 200);
    let (status, body) = post(
        &s,
        &bob,
        "/api/workspaces",
        json!({"id": "local-ws-1", "name": "Mine"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (409, Some("workspace_exists"))
    );

    // Members.
    assert_eq!(
        get(&s, Some(&bob), &format!("/api/workspaces/{ws}/members"))
            .await
            .0,
        403
    );
    let (status, m) = post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "bob", "role": "viewer"}),
    )
    .await;
    assert_eq!(status, 200, "{m}");
    assert_eq!(m["user_id"], bob_id.as_str());
    let (status, body) = post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "nobody", "role": "viewer"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (404, Some("user_not_found"))
    );
    let (status, _) = post(
        &s,
        &bob,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "alice", "role": "viewer"}),
    )
    .await;
    assert_eq!(status, 403);
    let (_, members) = get(&s, Some(&bob), &format!("/api/workspaces/{ws}/members")).await;
    assert_eq!(members.as_array().unwrap().len(), 2);
    let (_, bob_list) = get(&s, Some(&bob), "/api/workspaces").await;
    assert_eq!(bob_list[0]["role"], "viewer");

    // Removing a member.
    let res = http()
        .delete(format!("{}/api/workspaces/{ws}/members/{bob_id}", s.base))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 204);
    assert_eq!(
        get(&s, Some(&bob), &format!("/api/workspaces/{ws}/members"))
            .await
            .0,
        403
    );
}

#[tokio::test]
async fn sync_clients_converge_and_viewers_cannot_write() {
    let s = start().await;
    let (alice, _) = register(&s, "alice").await;
    let (bob, _) = register(&s, "bob").await;
    let (carol, _) = register(&s, "carol").await;
    let ws = create_workspace(&s, &alice, "Team").await;
    post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "bob", "role": "editor"}),
    )
    .await;
    post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "carol", "role": "viewer"}),
    )
    .await;

    // A non-member is refused at the handshake.
    let (dave, _) = register(&s, "dave").await;
    let outsider = Client::connect(&s, &dave, &ws);
    wait_until("outsider refused", || {
        outsider
            .host
            .status
            .lock()
            .unwrap()
            .contains(&SyncStatus::Unauthorized)
    })
    .await;
    outsider.handle.shutdown();

    let a = Client::connect(&s, &alice, &ws);
    let b = Client::connect(&s, &bob, &ws);
    let c = Client::connect(&s, &carol, &ws);
    wait_until("all online", || a.online() && b.online() && c.online()).await;
    for client in [&a, &b, &c] {
        client.handle.subscribe("page-1");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    a.type_text("page-1", "hello ");
    wait_until("bob sees alice", || b.text("page-1") == "hello ").await;
    b.type_text("page-1", "world");
    wait_until("alice sees bob", || a.text("page-1") == "hello world").await;
    wait_until("viewer sees both", || c.text("page-1") == "hello world").await;

    // The viewer's edit is refused and never reaches the editors.
    c.type_text("page-1", " (viewer)");
    wait_until("readonly error", || {
        c.host
            .errors
            .lock()
            .unwrap()
            .iter()
            .any(|(d, code)| d == "page-1" && code == "readonly")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.text("page-1"), "hello world");
    assert_eq!(b.text("page-1"), "hello world");

    // Offline edits converge on reconnect: a fresh client gets the full state.
    a.handle.shutdown();
    let a2 = Client::connect(&s, &alice, &ws);
    a2.handle.subscribe("page-1");
    wait_until("reconnected client has state", || {
        a2.text("page-1") == "hello world"
    })
    .await;

    b.type_text("page-1", "!");
    wait_until("a2 sees bang", || a2.text("page-1") == "hello world!").await;
}

#[tokio::test]
async fn awareness_is_relayed_and_cleared_on_disconnect() {
    let s = start().await;
    let (alice, _) = register(&s, "alice").await;
    let (bob, _) = register(&s, "bob").await;
    let ws = create_workspace(&s, &alice, "Team").await;
    post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "bob", "role": "viewer"}),
    )
    .await;

    let a = Client::connect(&s, &alice, &ws);
    let b = Client::connect(&s, &bob, &ws);
    wait_until("online", || a.online() && b.online()).await;
    a.handle.subscribe("p");
    b.handle.subscribe("p");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut clients = HashMap::new();
    clients.insert(
        ClientID::new(42),
        AwarenessUpdateEntry {
            clock: 1,
            json: r#"{"user":{"name":"alice"}}"#.into(),
        },
    );
    a.handle
        .send_awareness("p", AwarenessUpdate { clients }.encode_v1());
    wait_until("bob receives awareness", || {
        b.host.awareness.lock().unwrap().iter().any(|(d, u)| {
            d == "p"
                && AwarenessUpdate::decode_v1(u)
                    .map(|u| {
                        u.clients
                            .get(&ClientID::new(42))
                            .is_some_and(|e| e.json.contains("alice"))
                    })
                    .unwrap_or(false)
        })
    })
    .await;

    // A late subscriber gets the current states on subscribe.
    let late = Client::connect(&s, &bob, &ws);
    late.handle.subscribe("p");
    wait_until("late subscriber gets snapshot", || {
        !late.host.awareness.lock().unwrap().is_empty()
    })
    .await;

    a.handle.shutdown();
    wait_until("bob receives removal", || {
        b.host.awareness.lock().unwrap().iter().any(|(_, u)| {
            AwarenessUpdate::decode_v1(u)
                .map(|u| {
                    u.clients
                        .get(&ClientID::new(42))
                        .is_some_and(|e| e.json.as_ref() == "null")
                })
                .unwrap_or(false)
        })
    })
    .await;
}

// ---- MCP ---------------------------------------------------------------------

struct Mcp {
    url: String,
    token: String,
    session: Option<String>,
    next_id: u64,
}

fn parse_rpc(body: &str, id: u64) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        return v;
    }
    body.lines()
        .filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
        .find(|v| v["id"] == id)
        .unwrap_or_else(|| panic!("no response for id {id} in: {body}"))
}

impl Mcp {
    async fn connect(s: &Server, token: &str) -> Mcp {
        let mut m = Mcp {
            url: format!("{}/mcp", s.base),
            token: token.to_string(),
            session: None,
            next_id: 1,
        };
        let res = m
            .send(json!({
                "jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}
            }))
            .await;
        m.session = res
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().unwrap().to_string());
        let body = res.text().await.unwrap();
        let init = parse_rpc(&body, 0);
        assert_eq!(init["result"]["serverInfo"]["name"], "kawruh", "{body}");
        let res = m
            .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        assert!(res.status().is_success());
        m
    }

    async fn send(&self, body: Value) -> reqwest::Response {
        let mut req = http()
            .post(&self.url)
            .bearer_auth(&self.token)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .json(&body);
        if let Some(sid) = &self.session {
            req = req.header("Mcp-Session-Id", sid);
        }
        req.send().await.unwrap()
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let res = self
            .send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        let body = res.text().await.unwrap();
        parse_rpc(&body, id)
    }

    /// Calls a tool; returns (text, is_error).
    async fn call(&mut self, name: &str, args: Value) -> (String, bool) {
        let v = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await;
        let result = &v["result"];
        assert!(result.is_object(), "tool {name} failed: {v}");
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        (text, result["isError"].as_bool().unwrap_or(false))
    }
}

fn extract_id(text: &str) -> String {
    let start = text.rfind('(').unwrap() + 1;
    let end = text.rfind(')').unwrap();
    text[start..end].to_string()
}

#[tokio::test]
async fn mcp_tools_edit_live_documents() {
    let s = start().await;
    let (alice, _) = register(&s, "alice").await;
    let (bob, _) = register(&s, "bob").await;
    let ws = create_workspace(&s, &alice, "Agents").await;
    post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "bob", "role": "viewer"}),
    )
    .await;

    // Unauthenticated requests never reach the MCP service.
    let res = http()
        .post(format!("{}/mcp", s.base))
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 401);

    let (status, pat) = post(&s, &alice, "/api/tokens", json!({"name": "agent"})).await;
    assert_eq!(status, 200);
    let mut mcp = Mcp::connect(&s, pat["token"].as_str().unwrap()).await;

    let tools = mcp.rpc("tools/list", json!({})).await;
    let mut names: Vec<String> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "create_page",
            "delete_page",
            "list_pages",
            "list_workspaces",
            "move_page",
            "read_page",
            "rename_page",
            "search_pages",
            "update_page"
        ]
    );

    let (text, _) = mcp.call("list_workspaces", json!({})).await;
    assert!(text.contains(&ws), "{text}");

    // A desktop client is watching the workspace.
    let watcher = Client::connect(&s, &alice, &ws);
    wait_until("watcher online", || watcher.online()).await;
    watcher.handle.subscribe(&ws);

    let (text, err) = mcp
        .call(
            "create_page",
            json!({"workspace_id": ws, "title": "Plan", "markdown": "# Goals\n\n- ship **v1**\n\n```mermaid\ngraph TD\n  A --> B\n```\n"}),
        )
        .await;
    assert!(!err, "{text}");
    let page = extract_id(&text);
    wait_until("watcher sees new page in tree", || {
        kawruh_core::workspace::live_pages(&watcher.doc(&ws))
            .iter()
            .any(|p| p.id == page && p.title == "Plan")
    })
    .await;

    let (child_text, _) = mcp
        .call(
            "create_page",
            json!({"workspace_id": ws, "title": "Child", "parent_id": page}),
        )
        .await;
    let child = extract_id(&child_text);
    let (outline, _) = mcp.call("list_pages", json!({"workspace_id": ws})).await;
    assert!(
        outline.contains("- Plan (") && outline.contains("  - Child ("),
        "{outline}"
    );

    let (read, _) = mcp.call("read_page", json!({"page_id": page})).await;
    assert!(
        read.contains("Title: Plan")
            && read.contains("- ship **v1**")
            && read.contains("```mermaid"),
        "{read}"
    );

    // update_page reaches a subscribed client live.
    watcher.handle.subscribe(&page);
    wait_until("watcher has page content", || {
        kawruh_core::markdown::doc_to_markdown(&watcher.doc(&page)).contains("ship")
    })
    .await;
    let (_, err) = mcp
        .call(
            "update_page",
            json!({"page_id": page, "markdown": "Rewritten by the agent."}),
        )
        .await;
    assert!(!err);
    wait_until("watcher sees agent edit", || {
        kawruh_core::markdown::doc_to_markdown(&watcher.doc(&page)) == "Rewritten by the agent.\n"
    })
    .await;
    mcp.call(
        "update_page",
        json!({"page_id": page, "markdown": "And more.", "mode": "append"}),
    )
    .await;
    wait_until("watcher sees append", || {
        kawruh_core::markdown::doc_to_markdown(&watcher.doc(&page))
            == "Rewritten by the agent.\n\nAnd more.\n"
    })
    .await;

    mcp.call("rename_page", json!({"page_id": child, "title": "Renamed"}))
        .await;
    mcp.call("move_page", json!({"page_id": child, "parent_id": null}))
        .await;
    wait_until("watcher sees rename + move", || {
        kawruh_core::workspace::live_pages(&watcher.doc(&ws))
            .iter()
            .any(|p| p.id == child && p.title == "Renamed" && p.parent.is_empty())
    })
    .await;

    // Search is served from the debounced index.
    let mut found = String::new();
    for _ in 0..40 {
        found = mcp.call("search_pages", json!({"query": "agent"})).await.0;
        if found.contains(&page) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(found.contains(&page), "{found}");

    mcp.call("delete_page", json!({"page_id": child})).await;
    wait_until("watcher sees delete", || {
        !kawruh_core::workspace::live_pages(&watcher.doc(&ws))
            .iter()
            .any(|p| p.id == child)
    })
    .await;

    // Viewers cannot write through MCP either.
    let (_, bob_pat) = post(&s, &bob, "/api/tokens", json!({"name": "bob"})).await;
    let mut bob_mcp = Mcp::connect(&s, bob_pat["token"].as_str().unwrap()).await;
    let (text, err) = bob_mcp
        .call(
            "update_page",
            json!({"page_id": page, "markdown": "hijack"}),
        )
        .await;
    assert!(err && text.contains("read-only"), "{text}");
    let (read, err) = bob_mcp.call("read_page", json!({"page_id": page})).await;
    assert!(!err && read.contains("And more."), "{read}");
}

#[tokio::test]
async fn share_links_render_public_html() {
    let s = start().await;
    let (alice, _) = register(&s, "alice").await;
    let ws = create_workspace(&s, &alice, "Public").await;
    let (_, pat) = post(&s, &alice, "/api/tokens", json!({"name": "t"})).await;
    let mut mcp = Mcp::connect(&s, pat["token"].as_str().unwrap()).await;
    let (text, _) = mcp
        .call(
            "create_page",
            json!({"workspace_id": ws, "title": "Shared <Doc>", "markdown": "Hello **world** <script>alert(1)</script>\n\n```mermaid\ngraph TD\n  A --> B\n```\n\n```kawruh-canvas\nc1\n```\n"}),
        )
        .await;
    let page = extract_id(&text);
    let (child_text, _) = mcp.call("create_page", json!({"workspace_id": ws, "title": "Sub", "parent_id": page, "markdown": "child body"})).await;
    let child = extract_id(&child_text);
    let (other_text, _) = mcp
        .call(
            "create_page",
            json!({"workspace_id": ws, "title": "Private", "markdown": "secret"}),
        )
        .await;
    let other = extract_id(&other_text);

    let (status, share) = post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/shares"),
        json!({"page_id": page}),
    )
    .await;
    assert_eq!(status, 200, "{share}");
    let url = share["url"].as_str().unwrap().to_string();
    assert!(url.starts_with(&s.base));
    // Sharing the same page again returns the same link.
    let (_, again) = post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/shares"),
        json!({"page_id": page}),
    )
    .await;
    assert_eq!(again["token"], share["token"]);
    let (status, body) = post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/shares"),
        json!({"page_id": "nope"}),
    )
    .await;
    assert_eq!(
        (status, body["code"].as_str()),
        (404, Some("page_not_found"))
    );

    let html = http().get(&url).send().await.unwrap().text().await.unwrap();
    assert!(html.contains("<strong>world</strong>"), "{html}");
    assert!(
        !html.contains("<script>alert(1)"),
        "raw HTML must be escaped"
    );
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("Shared &lt;Doc&gt;"));
    assert!(html.contains("language-mermaid") && html.contains("mermaid.esm.min.mjs"));
    assert!(html.contains("Canvas drawing"));
    assert!(html.contains(&format!("/s/{}/{child}", share["token"].as_str().unwrap())));

    let sub = http().get(format!("{url}/{child}")).send().await.unwrap();
    assert_eq!(sub.status().as_u16(), 200);
    assert!(sub.text().await.unwrap().contains("child body"));
    // Pages outside the shared subtree are not reachable through the link.
    let private = http().get(format!("{url}/{other}")).send().await.unwrap();
    assert_eq!(private.status().as_u16(), 404);
    assert_eq!(
        http()
            .get(format!("{}/s/doesnotexist", s.base))
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        404
    );

    let (_, list) = get(&s, Some(&alice), &format!("/api/workspaces/{ws}/shares")).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let res = http()
        .delete(format!(
            "{}/api/shares/{}",
            s.base,
            share["token"].as_str().unwrap()
        ))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 204);
    assert_eq!(
        http().get(&url).send().await.unwrap().status().as_u16(),
        404
    );
}

#[tokio::test]
async fn documents_survive_a_restart() {
    let dir = Arc::new(tempfile::tempdir().unwrap());
    let s = start_in(dir.clone()).await;
    let (alice, _) = register(&s, "alice").await;
    let ws = create_workspace(&s, &alice, "Durable").await;
    let (_, pat) = post(&s, &alice, "/api/tokens", json!({"name": "t"})).await;
    let token = pat["token"].as_str().unwrap().to_string();
    let mut mcp = Mcp::connect(&s, &token).await;
    let (text, _) = mcp
        .call(
            "create_page",
            json!({"workspace_id": ws, "title": "Keep", "markdown": "persisted body"}),
        )
        .await;
    let page = extract_id(&text);
    // Let the event loop flush the updates to SQLite, then stop the server.
    tokio::time::sleep(Duration::from_millis(500)).await;
    s.task.abort();

    let s2 = start_in(dir).await;
    let mut mcp2 = Mcp::connect(&s2, &token).await;
    let (read, err) = mcp2.call("read_page", json!({"page_id": page})).await;
    assert!(
        !err && read.contains("Title: Keep") && read.contains("persisted body"),
        "{read}"
    );
    let (_, list) = get(&s2, Some(&alice), "/api/workspaces").await;
    assert_eq!(list[0]["name"], "Durable");
}

#[tokio::test]
async fn demoting_a_connected_editor_takes_effect_without_reconnect() {
    let s = start().await;
    let (alice, _) = register(&s, "alice").await;
    let (bob, _) = register(&s, "bob").await;
    let ws = create_workspace(&s, &alice, "Team").await;
    post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "bob", "role": "editor"}),
    )
    .await;

    let a = Client::connect(&s, &alice, &ws);
    let b = Client::connect(&s, &bob, &ws);
    wait_until("both online", || a.online() && b.online()).await;
    for client in [&a, &b] {
        client.handle.subscribe("page-1");
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    b.type_text("page-1", "editor ");
    wait_until("alice sees bob", || a.text("page-1") == "editor ").await;

    // Bob is demoted while his socket stays open; once the cached role
    // expires his writes are refused.
    post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": "bob", "role": "viewer"}),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(5200)).await;
    b.type_text("page-1", "after");
    wait_until("readonly after demotion", || {
        b.host
            .errors
            .lock()
            .unwrap()
            .iter()
            .any(|(d, code)| d == "page-1" && code == "readonly")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(a.text("page-1"), "editor ");
}

// ---- assets -------------------------------------------------------------------

fn asset_name(bytes: &[u8], ext: &str) -> String {
    format!("{}.{ext}", hex::encode(sha2::Sha256::digest(bytes)))
}

async fn put_asset(s: &Server, token: &str, ws: &str, name: &str, body: Vec<u8>) -> (u16, Value) {
    let res = http()
        .put(format!("{}/api/workspaces/{ws}/assets/{name}", s.base))
        .bearer_auth(token)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

fn header<'a>(res: &'a reqwest::Response, name: &str) -> &'a str {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap())
        .unwrap_or("")
}

async fn add_member(s: &Server, owner: &str, ws: &str, username: &str, role: &str) {
    let (status, body) = post(
        s,
        owner,
        &format!("/api/workspaces/{ws}/members"),
        json!({"username": username, "role": role}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn assets_round_trip_with_permissions() {
    let s = start().await;
    let (alice, _) = register(&s, "alice").await;
    let (bob, _) = register(&s, "bob").await;
    let (carol, _) = register(&s, "carol").await;
    let (mallory, _) = register(&s, "mallory").await;
    let ws = create_workspace(&s, &alice, "Pictures").await;
    add_member(&s, &alice, &ws, "bob", "viewer").await;
    add_member(&s, &alice, &ws, "carol", "editor").await;

    // Larger than axum's default 2 MB body limit, below KAWRUH_MAX_ASSET_MB.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.resize(3 * 1024 * 1024, 7);
    let name = asset_name(&png, "png");
    let (status, body) = put_asset(&s, &alice, &ws, &name, png.clone()).await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body, json!({"name": name, "size": png.len()}));
    // Uploading the same content again is idempotent (an editor this time).
    let (status, body) = put_asset(&s, &carol, &ws, &name, png.clone()).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, json!({"name": name, "size": png.len()}));
    // Personal access tokens work too.
    let (_, pat) = post(&s, &alice, "/api/tokens", json!({"name": "app"})).await;
    let pat = pat["token"].as_str().unwrap().to_string();
    let jpg = b"\xff\xd8\xff not really a jpeg".to_vec();
    let jpg_name = asset_name(&jpg, "jpg");
    assert_eq!(
        put_asset(&s, &pat, &ws, &jpg_name, jpg.clone()).await.0,
        201
    );

    // Any member can download, with immutable caching and nosniff.
    let url = |n: &str| format!("{}/api/workspaces/{ws}/assets/{n}", s.base);
    let res = http()
        .get(url(&name))
        .bearer_auth(&bob)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(header(&res, "content-type"), "image/png");
    assert_eq!(
        header(&res, "cache-control"),
        "private, max-age=31536000, immutable"
    );
    assert_eq!(header(&res, "x-content-type-options"), "nosniff");
    assert_eq!(res.bytes().await.unwrap().as_ref(), png.as_slice());
    let res = http()
        .get(url(&jpg_name))
        .bearer_auth(&pat)
        .send()
        .await
        .unwrap();
    assert_eq!(header(&res, "content-type"), "image/jpeg");
    assert_eq!(res.bytes().await.unwrap().as_ref(), jpg.as_slice());

    // Stored under <data>/assets/<ws>/, with no temp files left behind.
    let dir = s.dir.path().join("assets").join(&ws);
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    let mut expected = vec![name.clone(), jpg_name.clone()];
    expected.sort();
    assert_eq!(files, expected);

    // Content that does not match the hash in the name.
    let wrong = asset_name(b"something else", "png");
    let (status, body) = put_asset(&s, &alice, &ws, &wrong, b"tampered".to_vec()).await;
    assert_eq!((status, body["code"].as_str()), (400, Some("bad_hash")));
    assert!(!dir.join(&wrong).exists());

    // Malformed names.
    for bad in [
        "foo.png".to_string(),
        format!("{}.svg", &name[..64]),
        format!("{}.png", name[..64].to_uppercase()),
        format!("{}.png.png", &name[..64]),
    ] {
        let (status, body) = put_asset(&s, &alice, &ws, &bad, png.clone()).await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("bad_name")),
            "{bad}"
        );
        let (status, body) = get(
            &s,
            Some(&alice),
            &format!("/api/workspaces/{ws}/assets/{bad}"),
        )
        .await;
        assert_eq!(
            (status, body["code"].as_str()),
            (400, Some("bad_name")),
            "{bad}"
        );
    }
    let missing = asset_name(b"never uploaded", "webp");
    let (status, body) = get(
        &s,
        Some(&bob),
        &format!("/api/workspaces/{ws}/assets/{missing}"),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (404, Some("not_found")));

    // Viewers cannot upload; non-members can do neither.
    // (Refused uploads still get a readable error for a multi-MB body.)
    let small = b"GIF89a".to_vec();
    let small_name = asset_name(&small, "gif");
    let (status, body) = put_asset(&s, &bob, &ws, &name, png.clone()).await;
    assert_eq!((status, body["code"].as_str()), (403, Some("forbidden")));
    let (status, body) = put_asset(&s, "not-a-token", &ws, &name, png.clone()).await;
    assert_eq!((status, body["code"].as_str()), (401, Some("unauthorized")));
    let (status, body) = put_asset(&s, &mallory, &ws, &small_name, small.clone()).await;
    assert_eq!((status, body["code"].as_str()), (403, Some("forbidden")));
    let (status, body) = get(
        &s,
        Some(&mallory),
        &format!("/api/workspaces/{ws}/assets/{name}"),
    )
    .await;
    assert_eq!((status, body["code"].as_str()), (403, Some("forbidden")));
    let (status, _) = get(&s, None, &format!("/api/workspaces/{ws}/assets/{name}")).await;
    assert_eq!(status, 401);
    let (status, body) = put_asset(&s, &alice, "no-such-ws", &small_name, small.clone()).await;
    assert_eq!(
        (status, body["code"].as_str()),
        (404, Some("workspace_not_found"))
    );
    assert!(!dir.join(&small_name).exists());

    // Deleting the workspace removes its assets.
    let res = http()
        .delete(format!("{}/api/workspaces/{ws}", s.base))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 204);
    assert!(!dir.exists());
}

/// Sends only the request head (no body) and returns the raw response, so an
/// early 413 can be read without the client racing a connection reset.
async fn raw_put_head(s: &Server, token: &str, path: &str, content_length: u64) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = s.base.trim_start_matches("http://");
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "PUT {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
    );
    sock.write_all(head.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), sock.read_to_end(&mut buf))
        .await
        .expect("no response to request head")
        .unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn asset_uploads_respect_size_and_disk_limits() {
    let s = start_with(Arc::new(tempfile::tempdir().unwrap()), |c| {
        c.max_asset_mb = 1
    })
    .await;
    let (alice, _) = register(&s, "alice").await;
    let ws = create_workspace(&s, &alice, "Limits").await;

    let exact = vec![1u8; 1024 * 1024];
    let (status, body) = put_asset(&s, &alice, &ws, &asset_name(&exact, "gif"), exact).await;
    assert_eq!(status, 201, "{body}");

    let over = vec![1u8; 1024 * 1024 + 1];
    let path = format!("/api/workspaces/{ws}/assets/{}", asset_name(&over, "gif"));
    let res = raw_put_head(&s, &alice, &path, over.len() as u64).await;
    assert!(res.starts_with("HTTP/1.1 413"), "{res}");
    assert!(res.contains("\"code\":\"too_large\""), "{res}");

    // Below KAWRUH_MIN_FREE_MB, uploads get the low-disk refusal.
    let low = start_with(Arc::new(tempfile::tempdir().unwrap()), |c| {
        c.min_free_mb = 1 << 40
    })
    .await;
    let (alice, _) = register(&low, "alice").await;
    let ws = create_workspace(&low, &alice, "Full").await;
    let png = b"png".to_vec();
    let (status, body) = put_asset(&low, &alice, &ws, &asset_name(&png, "png"), png).await;
    assert_eq!((status, body["code"].as_str()), (507, Some("disk_full")));
}

#[tokio::test]
async fn share_pages_serve_assets_and_link_cards() {
    let s = start().await;
    let (alice, _) = register(&s, "alice").await;
    let ws = create_workspace(&s, &alice, "Blog").await;
    let elsewhere = create_workspace(&s, &alice, "Elsewhere").await;
    let img = b"GIF89a shared".to_vec();
    let img_name = asset_name(&img, "gif");
    assert_eq!(
        put_asset(&s, &alice, &ws, &img_name, img.clone()).await.0,
        201
    );
    let foreign = b"RIFF webp elsewhere".to_vec();
    let foreign_name = asset_name(&foreign, "webp");
    assert_eq!(
        put_asset(&s, &alice, &elsewhere, &foreign_name, foreign)
            .await
            .0,
        201
    );

    let markdown = format!(
        "Intro.\n\n![diagram](kawruh-asset://{ws}/{img_name})\n\n![foreign](kawruh-asset://{elsewhere}/{foreign_name})\n\n[Example Domain](https://example.com \"kawruh:card\")\n\n[Big Post](https://blog.example/post \"kawruh:preview\")\n\n[Evil link](javascript:alert(3) \"kawruh:card\")\n"
    );
    let (_, pat) = post(&s, &alice, "/api/tokens", json!({"name": "t"})).await;
    let mut mcp = Mcp::connect(&s, pat["token"].as_str().unwrap()).await;
    let (text, err) = mcp
        .call(
            "create_page",
            json!({"workspace_id": ws, "title": "Post", "markdown": markdown}),
        )
        .await;
    assert!(!err, "{text}");
    let page = extract_id(&text);
    let (status, share) = post(
        &s,
        &alice,
        &format!("/api/workspaces/{ws}/shares"),
        json!({"page_id": page}),
    )
    .await;
    assert_eq!(status, 200, "{share}");
    let token = share["token"].as_str().unwrap().to_string();
    let url = share["url"].as_str().unwrap().to_string();

    let res = http().get(&url).send().await.unwrap();
    let csp = header(&res, "content-security-policy").to_string();
    assert!(
        csp.contains("default-src 'none'") && csp.contains("img-src 'self' data: https: http:"),
        "{csp}"
    );
    let html = res.text().await.unwrap();
    // Images of this workspace point at the share's asset route; others stay as-is.
    assert!(
        html.contains(&format!(
            "<img src=\"/s/{token}/assets/{img_name}\" alt=\"diagram\""
        )),
        "{html}"
    );
    assert!(html.contains(&format!(
        "src=\"kawruh-asset://{elsewhere}/{foreign_name}\""
    )));
    // Cards without metadata: title + domain.
    assert!(html.contains(
        "<a class=\"link-card\" href=\"https://example.com\" rel=\"noopener noreferrer nofollow\">"
    ));
    assert!(html.contains("<span class=\"lc-title\">Example Domain</span>"));
    assert!(html.contains("<span>example.com</span>"));
    assert!(
        html.contains(
            "<a class=\"link-card link-card-preview\" href=\"https://blog.example/post\""
        )
    );
    // A javascript: card is not a link at all.
    assert!(html.contains("<p>Evil link</p>"), "{html}");
    assert!(!html.contains("javascript:"));

    // The share's asset route serves only the shared workspace's assets.
    let res = http()
        .get(format!("{url}/assets/{img_name}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 200);
    assert_eq!(header(&res, "content-type"), "image/gif");
    assert_eq!(
        header(&res, "cache-control"),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(header(&res, "x-content-type-options"), "nosniff");
    assert_eq!(res.bytes().await.unwrap().as_ref(), img.as_slice());
    for path in [
        format!("{url}/assets/{foreign_name}"),
        format!("{url}/assets/{}", asset_name(b"missing", "png")),
        format!("{url}/assets/not-a-hash.png"),
        format!("{}/s/doesnotexist/assets/{img_name}", s.base),
    ] {
        let status = http().get(&path).send().await.unwrap().status().as_u16();
        assert_eq!(status, 404, "{path}");
    }

    // Seed link metadata in the page doc's `link_meta` map, as the app does.
    let c = Client::connect(&s, &alice, &ws);
    wait_until("client online", || c.online()).await;
    c.handle.subscribe(&page);
    wait_until("page synced", || {
        kawruh_core::markdown::doc_to_markdown(&c.doc(&page)).contains("Example Domain")
    })
    .await;
    let doc = c.doc(&page);
    let sv = doc.transact().state_vector();
    let link_meta = doc.get_or_insert_map("link_meta");
    {
        let mut txn = doc.transact_mut();
        link_meta.insert(
            &mut txn,
            "https://example.com",
            json!({
                "title": "Example <script>alert(1)</script>",
                "description": "An \"example\" & more",
                "image": "https://example.com/og.png",
                "icon": "javascript:alert(2)",
                "site": "Example"
            })
            .to_string(),
        );
        link_meta.insert(
            &mut txn,
            "https://blog.example/post",
            json!({
                "title": "A big post",
                "image": "https://blog.example/cover.jpg",
                "icon": "https://blog.example/favicon.ico"
            })
            .to_string(),
        );
    }
    let update = doc.transact().encode_state_as_update_v1(&sv);
    c.handle.send_update(&page, update);

    let mut html = String::new();
    for _ in 0..50 {
        html = http().get(&url).send().await.unwrap().text().await.unwrap();
        if html.contains("A big post") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        html.contains(
            "<span class=\"lc-title\">Example &lt;script&gt;alert(1)&lt;/script&gt;</span>"
        ),
        "{html}"
    );
    assert!(!html.contains("<script>alert(1)"));
    assert!(html.contains("<span class=\"lc-desc\">An &quot;example&quot; &amp; more</span>"));
    assert!(html.contains("<img src=\"https://example.com/og.png\""));
    assert!(html.contains("<span>Example</span>"));
    assert!(!html.contains("javascript:"), "{html}");
    assert!(html.contains("<span class=\"lc-title\">A big post</span>"));
    assert!(html.contains("<img src=\"https://blog.example/cover.jpg\""));
    assert!(html.contains("<img class=\"lc-icon\" src=\"https://blog.example/favicon.ico\""));
    assert!(html.contains("<span>blog.example</span>"));

    // Revoking the link also closes its asset route.
    let res = http()
        .delete(format!("{}/api/shares/{token}", s.base))
        .bearer_auth(&alice)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status().as_u16(), 204);
    let status = http()
        .get(format!("{url}/assets/{img_name}"))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(status, 404);
}
