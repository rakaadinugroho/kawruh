//! Operator commands run against the database directly:
//! `kawruh-server admin list-users` and `kawruh-server admin delete-user <name>`.
//! Stop the service first so no live document is written back afterwards.

use rusqlite::{Connection, params};

pub fn list_users(conn: &Connection) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT u.username, u.display_name,
                (SELECT COUNT(*) FROM workspaces w WHERE w.owner_id = u.id)
         FROM users u ORDER BY u.created_at",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

/// Deletes a user, the workspaces they own (documents included) and their
/// tokens and memberships. Returns how many workspaces were removed.
pub fn delete_user(conn: &mut Connection, username: &str) -> anyhow::Result<usize> {
    let user_id: String = conn
        .query_row(
            "SELECT id FROM users WHERE username = ?1",
            [username],
            |r| r.get(0),
        )
        .map_err(|_| anyhow::anyhow!("no user named {username}"))?;
    let owned: Vec<String> = {
        let mut stmt = conn.prepare("SELECT id FROM workspaces WHERE owner_id = ?1")?;
        let rows = stmt.query_map([&user_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for ws in &owned {
        kawruh_core::store::delete_workspace_docs(conn, ws)?;
        conn.execute("DELETE FROM page_text WHERE workspace_id = ?1", [ws])?;
        conn.execute("DELETE FROM workspaces WHERE id = ?1", [ws])?;
    }
    conn.execute("DELETE FROM users WHERE id = ?1", params![user_id])?;
    Ok(owned.len())
}

pub fn run(args: &[String], conn: &mut Connection) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("list-users") => {
            for (name, display, owned) in list_users(conn)? {
                println!("{name}\t{display}\t{owned} workspace(s)");
            }
            Ok(())
        }
        Some("delete-user") => {
            let name = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: admin delete-user <username>"))?;
            let n = delete_user(conn, name)?;
            println!("deleted {name} and {n} owned workspace(s)");
            Ok(())
        }
        _ => anyhow::bail!("usage: kawruh-server admin (list-users | delete-user <username>)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_user_removes_owned_workspaces() {
        let dir = std::env::temp_dir().join(format!("kawruh-admin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Db::open(&dir.join("k.db")).unwrap();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO users (id, username, display_name, password_hash, created_at) VALUES ('u1','alice','Alice','x',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO workspaces (id, name, owner_id, created_at) VALUES ('w1','W','u1',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO members (workspace_id, user_id, role) VALUES ('w1','u1','owner')",
                [],
            )
            .unwrap();
            kawruh_core::store::ensure_doc(
                &conn,
                "w1",
                "w1",
                kawruh_core::store::DocKind::Workspace,
            )
            .unwrap();
        }
        let mut conn = db.lock();
        assert_eq!(list_users(&conn).unwrap().len(), 1);
        assert_eq!(delete_user(&mut conn, "alice").unwrap(), 1);
        assert!(list_users(&conn).unwrap().is_empty());
        let docs: i64 = conn
            .query_row("SELECT COUNT(*) FROM docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(docs, 0);
        assert!(delete_user(&mut conn, "nobody").is_err());
    }
}
