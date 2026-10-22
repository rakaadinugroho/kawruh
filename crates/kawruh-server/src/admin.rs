//! Operator commands run against the database directly:
//! `kawruh-server admin list-users`, `kawruh-server admin delete-user <name>`,
//! `kawruh-server admin invite` and `kawruh-server admin gc [--dry-run]`.
//! Stop the service before `delete-user` so no live document is written back
//! afterwards; `invite` and `gc` are safe while it runs.

use std::path::Path;

use rusqlite::{Connection, params};

use crate::db::Db;

pub fn list_users(conn: &Connection) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT u.username, u.display_name,
                (SELECT COUNT(*) FROM workspaces w WHERE w.owner_id = u.id)
         FROM users u ORDER BY u.created_at",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

/// A fresh system invite code (no owner), valid for 7 days.
pub fn create_invite(conn: &Connection) -> rusqlite::Result<String> {
    let invite = crate::db::create_invite(conn, None, kawruh_core::workspace::now_ms())?;
    Ok(invite.code)
}

/// Deletes a user, the workspaces they own (documents and assets included)
/// and their tokens and memberships. Returns how many workspaces were removed.
pub fn delete_user(
    conn: &mut Connection,
    data_dir: &Path,
    username: &str,
) -> anyhow::Result<usize> {
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
    for ws in &owned {
        crate::assets::remove_workspace(data_dir, ws)
            .map_err(|e| anyhow::anyhow!("removing assets of workspace {ws}: {e}"))?;
    }
    Ok(owned.len())
}

pub fn run(args: &[String], db: &Db, data_dir: &Path) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        Some("list-users") => {
            let conn = db.lock();
            for (name, display, owned) in list_users(&conn)? {
                println!("{name}\t{display}\t{owned} workspace(s)");
            }
            Ok(())
        }
        Some("delete-user") => {
            let name = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("usage: admin delete-user <username>"))?;
            let mut conn = db.lock();
            let n = delete_user(&mut conn, data_dir, name)?;
            println!("deleted {name} and {n} owned workspace(s)");
            Ok(())
        }
        Some("invite") => {
            let conn = db.lock();
            println!("{}", create_invite(&conn)?);
            Ok(())
        }
        Some("gc") => {
            let dry_run = args.iter().any(|a| a == "--dry-run");
            let report = crate::gc::sweep(
                db,
                data_dir,
                None,
                kawruh_core::workspace::now_ms(),
                dry_run,
            )?;
            print!("{}", report.render());
            Ok(())
        }
        _ => anyhow::bail!(
            "usage: kawruh-server admin (list-users | delete-user <username> | invite | gc [--dry-run])"
        ),
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
        let assets = crate::assets::workspace_dir(&dir, "w1");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::write(assets.join("a.png"), b"x").unwrap();
        let mut conn = db.lock();
        assert_eq!(list_users(&conn).unwrap().len(), 1);
        assert_eq!(delete_user(&mut conn, &dir, "alice").unwrap(), 1);
        assert!(!assets.exists());
        assert!(list_users(&conn).unwrap().is_empty());
        let docs: i64 = conn
            .query_row("SELECT COUNT(*) FROM docs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(docs, 0);
        assert!(delete_user(&mut conn, &dir, "nobody").is_err());
    }

    #[test]
    fn gc_command_sweeps_without_touching_anything() {
        let dir = std::env::temp_dir().join(format!("kawruh-admin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = crate::db::Db::open(&dir.join("k.db")).unwrap();
        // An image no page refers to.
        let assets = crate::assets::workspace_dir(&dir, "w1");
        std::fs::create_dir_all(&assets).unwrap();
        let img = assets.join(format!("{}.png", "a".repeat(64)));
        std::fs::write(&img, b"x").unwrap();

        // A dry run reports it and leaves the file alone.
        run(&["gc".to_string(), "--dry-run".to_string()], &db, &dir).unwrap();
        assert!(img.exists());

        // Without the flag it is marked, and only deleted after the grace period.
        run(&["gc".to_string()], &db, &dir).unwrap();
        assert!(img.exists());
        let orphans: i64 = db
            .lock()
            .query_row("SELECT COUNT(*) FROM orphan_assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphans, 1);
    }
}
