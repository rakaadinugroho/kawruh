//! The desktop app's backend for `kawruh-mcp`'s tools. It serves the local
//! data only: workspaces that never synced to a server (`account_id IS NULL`)
//! stay theirs to browse and edit, documents come from the open `DocHub`, so
//! MCP edits land in the same page an editor may have open right now, and
//! images go straight into SQLite. Server-backed workspaces stay out: with a
//! kawruh-server connected, its own MCP endpoint covers them.

use std::sync::Arc;

use kawruh_core::mcp::{McpBackend, McpCaller, McpHit, McpPageRef, McpWorkspace};
use kawruh_core::yrs::Doc;

use crate::db;
use crate::hub::DocHub;
use crate::search::SearchIndex;

pub struct DesktopBackend {
    pub hub: Arc<DocHub>,
    pub search: Arc<SearchIndex>,
}

/// A local store sits behind one user, and comment authors read "You",
/// matching what the local account writes (`PageView`, id "local").
pub fn local_caller() -> McpCaller {
    McpCaller {
        id: "local".into(),
        name: "You".into(),
    }
}

impl McpBackend for DesktopBackend {
    fn workspaces(&self, _caller: &McpCaller) -> Result<Vec<McpWorkspace>, String> {
        let conn = self.hub.db.lock().unwrap();
        Ok(db::list_workspaces(&conn)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|w| w.account_id.is_none())
            .map(|w| McpWorkspace {
                id: w.id,
                name: w.name,
                may_write: true,
            })
            .collect())
    }

    fn require_write(&self, _caller: &McpCaller, workspace_id: &str) -> Result<(), String> {
        let conn = self.hub.db.lock().unwrap();
        match db::get_workspace(&conn, workspace_id).map_err(|e| e.to_string())? {
            Some(w) if w.account_id.is_none() => Ok(()),
            Some(_) => Err(format!(
                "workspace {workspace_id} syncs to a kawruh-server; connect this computer's \
                 agents to the server's MCP endpoint to reach it"
            )),
            None => Err(format!("workspace {workspace_id} not found")),
        }
    }

    fn doc(
        &self,
        _caller: &McpCaller,
        doc_id: &str,
        workspace_id: &str,
        _writable: bool,
    ) -> Result<Doc, String> {
        self.hub
            .get_or_load(doc_id, workspace_id)
            .map_err(|e| e.to_string())
    }

    fn page_workspace(&self, page_id: &str) -> Result<Option<String>, String> {
        let conn = self.hub.db.lock().unwrap();
        kawruh_core::store::doc_workspace(&conn, page_id).map_err(|e| e.to_string())
    }

    fn search(
        &self,
        _caller: &McpCaller,
        workspace_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<McpHit>, String> {
        let mut hits = Vec::new();
        for ws in workspace_ids {
            for h in self
                .search
                .search(&self.hub, ws, query, limit)
                .map_err(|e| e.to_string())?
            {
                hits.push(McpHit {
                    workspace_id: ws.clone(),
                    page_id: h.id,
                    title: h.title,
                    snippet: h.snippet,
                });
            }
        }
        Ok(hits)
    }

    fn backlinks(
        &self,
        _caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<Vec<McpPageRef>, String> {
        Ok(self
            .search
            .backlinks(&self.hub, workspace_id, page_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|r| McpPageRef {
                page_id: r.id,
                title: r.title,
            })
            .collect())
    }

    fn store_asset(
        &self,
        _caller: &McpCaller,
        workspace_id: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<String, String> {
        let conn = self.hub.db.lock().unwrap();
        crate::assets::put(&conn, workspace_id, bytes, mime).map_err(|e| e.to_string())
    }

    fn index_page(
        &self,
        _caller: &McpCaller,
        _workspace_id: &str,
        page_id: &str,
        _title: &str,
        _markdown: &str,
        _is_database: bool,
    ) -> Result<(), String> {
        // The search index refreshes live docs on demand; newest writes just
        // need marking so the next search sees them right away.
        self.search.mark(page_id);
        Ok(())
    }

    fn share_page(
        &self,
        _caller: &McpCaller,
        _workspace_id: &str,
        _page_id: &str,
    ) -> Result<String, String> {
        Err(
            "this Kawruh has no public page sharing; publish the workspace to a kawruh-server \
             and connect the agents to that server's MCP endpoint to share"
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex;

    fn backend(workspaces: &[(&str, Option<&str>)]) -> DesktopBackend {
        let conn = Arc::new(Mutex::new(Connection::open_in_memory().unwrap()));
        db::migrate(&conn.lock().unwrap()).unwrap();
        let (hub, _rx) = DocHub::new(Arc::clone(&conn));
        for (i, (name, account)) in workspaces.iter().enumerate() {
            db::insert_workspace(
                &conn.lock().unwrap(),
                &format!("w{i}"),
                name,
                *account,
                "owner",
            )
            .unwrap();
        }
        DesktopBackend {
            hub,
            search: Arc::new(SearchIndex::default()),
        }
    }

    #[test]
    fn lists_local_workspaces_only() {
        let b = backend(&[("Solo", None), ("Synced", Some("a1"))]);
        let names: Vec<String> = b
            .workspaces(&local_caller())
            .unwrap()
            .into_iter()
            .map(|w| w.name)
            .collect();
        assert_eq!(names, vec!["Solo".to_string()]);
    }

    #[test]
    fn refuses_writes_into_server_workspaces() {
        let b = backend(&[("Solo", None), ("Synced", Some("a1"))]);
        b.require_write(&local_caller(), "w0").unwrap();
        assert!(
            b.require_write(&local_caller(), "w1")
                .unwrap_err()
                .contains("kawruh-server")
        );
        assert!(b.require_write(&local_caller(), "w9").is_err());
    }
}
