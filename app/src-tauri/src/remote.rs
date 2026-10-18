//! HTTP client for the server REST API (docs/API.md). The webview never talks
//! to the server directly; tokens stay on the Rust side.

use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Error, Result};

#[derive(Clone)]
pub struct Remote {
    http: reqwest::Client,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RemoteWorkspace {
    pub id: String,
    pub name: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Member {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub role: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct Share {
    pub token: String,
    pub page_id: String,
    pub url: String,
    #[serde(default)]
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct ApiToken {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    /// Only present right after creation.
    #[serde(default)]
    pub token: Option<String>,
}

pub fn normalize_server(url: &str) -> String {
    let url = url.trim().trim_end_matches('/');
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("http://{url}")
    }
}

impl Remote {
    pub fn new() -> Remote {
        Remote {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("http client"),
        }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        server: &str,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Result<T> {
        let mut req = self.http.request(method, format!("{server}{path}"));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await?;
        let status = res.status();
        if status.is_success() {
            if status == reqwest::StatusCode::NO_CONTENT {
                return serde_json::from_value(Value::Null).map_err(|e| Error::msg(e.to_string()));
            }
            return Ok(res.json().await?);
        }
        let body: Value = res.json().await.unwrap_or_default();
        Err(Error::Remote {
            status: status.as_u16(),
            code: body["code"].as_str().unwrap_or("http_error").to_string(),
            message: body["error"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("server returned {status}")),
        })
    }

    pub async fn auth(
        &self,
        server: &str,
        username: &str,
        password: &str,
        register: bool,
        display_name: Option<&str>,
    ) -> Result<AuthResponse> {
        let path = if register {
            "/api/auth/register"
        } else {
            "/api/auth/login"
        };
        let mut body = json!({ "username": username, "password": password });
        if let Some(d) = display_name.filter(|d| !d.trim().is_empty()) {
            body["display_name"] = json!(d.trim());
        }
        self.call(Method::POST, server, path, None, Some(body))
            .await
    }

    pub async fn logout(&self, server: &str, token: &str) -> Result<()> {
        self.call(Method::POST, server, "/api/auth/logout", Some(token), None)
            .await
    }

    pub async fn workspaces(&self, server: &str, token: &str) -> Result<Vec<RemoteWorkspace>> {
        self.call(Method::GET, server, "/api/workspaces", Some(token), None)
            .await
    }

    pub async fn create_workspace(
        &self,
        server: &str,
        token: &str,
        id: Option<&str>,
        name: &str,
    ) -> Result<RemoteWorkspace> {
        let mut body = json!({ "name": name });
        if let Some(id) = id {
            body["id"] = json!(id);
        }
        self.call(
            Method::POST,
            server,
            "/api/workspaces",
            Some(token),
            Some(body),
        )
        .await
    }

    pub async fn members(&self, server: &str, token: &str, ws: &str) -> Result<Vec<Member>> {
        self.call(
            Method::GET,
            server,
            &format!("/api/workspaces/{ws}/members"),
            Some(token),
            None,
        )
        .await
    }

    pub async fn add_member(
        &self,
        server: &str,
        token: &str,
        ws: &str,
        username: &str,
        role: &str,
    ) -> Result<Member> {
        self.call(
            Method::POST,
            server,
            &format!("/api/workspaces/{ws}/members"),
            Some(token),
            Some(json!({ "username": username, "role": role })),
        )
        .await
    }

    pub async fn remove_member(
        &self,
        server: &str,
        token: &str,
        ws: &str,
        user_id: &str,
    ) -> Result<()> {
        self.call(
            Method::DELETE,
            server,
            &format!("/api/workspaces/{ws}/members/{user_id}"),
            Some(token),
            None,
        )
        .await
    }

    pub async fn shares(&self, server: &str, token: &str, ws: &str) -> Result<Vec<Share>> {
        self.call(
            Method::GET,
            server,
            &format!("/api/workspaces/{ws}/shares"),
            Some(token),
            None,
        )
        .await
    }

    pub async fn create_share(
        &self,
        server: &str,
        token: &str,
        ws: &str,
        page_id: &str,
    ) -> Result<Share> {
        self.call(
            Method::POST,
            server,
            &format!("/api/workspaces/{ws}/shares"),
            Some(token),
            Some(json!({ "page_id": page_id })),
        )
        .await
    }

    pub async fn delete_share(&self, server: &str, token: &str, share: &str) -> Result<()> {
        self.call(
            Method::DELETE,
            server,
            &format!("/api/shares/{share}"),
            Some(token),
            None,
        )
        .await
    }

    pub async fn tokens(&self, server: &str, token: &str) -> Result<Vec<ApiToken>> {
        self.call(Method::GET, server, "/api/tokens", Some(token), None)
            .await
    }

    pub async fn create_token(&self, server: &str, token: &str, name: &str) -> Result<ApiToken> {
        self.call(
            Method::POST,
            server,
            "/api/tokens",
            Some(token),
            Some(json!({ "name": name })),
        )
        .await
    }

    pub async fn delete_token(&self, server: &str, token: &str, id: &str) -> Result<()> {
        self.call(
            Method::DELETE,
            server,
            &format!("/api/tokens/{id}"),
            Some(token),
            None,
        )
        .await
    }
}
