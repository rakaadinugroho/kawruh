//! The workspace document: page tree + workspace metadata.
//!
//! Layout (mirrored by `app/src/lib/workspace.ts`):
//! `meta: Map{ name }`
//! `pages: Map<pageId, Map{ title, parent ("" = root), order (f64), icon,
//!                          created (ms), deleted (ms, 0 = live),
//!                          kind ("" = page, "database" = database) }>`

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use yrs::{Any, Doc, Map, MapPrelim, MapRef, Out, ReadTxn, Transact, TransactionMut};

pub const PAGES: &str = "pages";
pub const META: &str = "meta";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PageMeta {
    pub id: String,
    pub title: String,
    pub parent: String,
    pub order: f64,
    pub icon: String,
    pub created: i64,
    pub deleted: i64,
    /// "database" for a database page; empty for an ordinary page.
    pub kind: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("page {0} not found")]
    NotFound(String),
    #[error("cannot move a page inside itself")]
    Cycle,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn str_of<T: ReadTxn>(m: &MapRef, txn: &T, key: &str) -> String {
    match m.get(txn, key) {
        Some(Out::Any(Any::String(s))) => s.to_string(),
        _ => String::new(),
    }
}

fn num_of<T: ReadTxn>(m: &MapRef, txn: &T, key: &str) -> f64 {
    match m.get(txn, key) {
        Some(Out::Any(Any::Number(n))) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn read_pages<T: ReadTxn>(pages: &MapRef, txn: &T) -> Vec<PageMeta> {
    let mut out: Vec<PageMeta> = pages
        .iter(txn)
        .filter_map(|(id, v)| match v {
            Out::YMap(m) => Some(PageMeta {
                id: id.to_string(),
                title: str_of(&m, txn, "title"),
                parent: str_of(&m, txn, "parent"),
                order: num_of(&m, txn, "order"),
                icon: str_of(&m, txn, "icon"),
                created: num_of(&m, txn, "created") as i64,
                deleted: num_of(&m, txn, "deleted") as i64,
                kind: str_of(&m, txn, "kind"),
            }),
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.order.total_cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    out
}

/// All pages, including deleted ones, sorted by `order`.
pub fn all_pages(doc: &Doc) -> Vec<PageMeta> {
    let pages = doc.get_or_insert_map(PAGES);
    let txn = doc.transact();
    break_cycles(read_pages(&pages, &txn))
}

/// Concurrent moves (X under Y here, Y under X elsewhere) can merge into a
/// parent loop, which would make every page in it unreachable. The smallest
/// id of each loop is shown at the root instead. Mirrored in workspace.ts.
pub fn break_cycles(mut pages: Vec<PageMeta>) -> Vec<PageMeta> {
    let index: HashMap<String, usize> = pages
        .iter()
        .enumerate()
        .map(|(i, p)| (p.id.clone(), i))
        .collect();
    let mut breakers: HashSet<usize> = HashSet::new();
    for start in 0..pages.len() {
        let mut path: Vec<usize> = Vec::new();
        let mut cur = start;
        loop {
            if breakers.contains(&cur) {
                break;
            }
            if let Some(pos) = path.iter().position(|&i| i == cur) {
                let min = path[pos..]
                    .iter()
                    .copied()
                    .min_by(|&a, &b| pages[a].id.cmp(&pages[b].id))
                    .unwrap_or(cur);
                breakers.insert(min);
                break;
            }
            path.push(cur);
            match index.get(&pages[cur].parent) {
                Some(&parent) => cur = parent,
                None => break,
            }
        }
    }
    for i in breakers {
        pages[i].parent.clear();
    }
    pages
}

/// Pages that are neither deleted nor under a deleted (or missing / cyclic) ancestor.
pub fn live_pages(doc: &Doc) -> Vec<PageMeta> {
    live(all_pages(doc))
}

pub fn live(all: Vec<PageMeta>) -> Vec<PageMeta> {
    let by_id: HashMap<&str, &PageMeta> = all.iter().map(|p| (p.id.as_str(), p)).collect();
    let is_live = |p: &PageMeta| {
        let mut seen = HashSet::new();
        let mut cur = p;
        loop {
            if cur.deleted != 0 || !seen.insert(cur.id.as_str()) {
                return false;
            }
            if cur.parent.is_empty() {
                return true;
            }
            match by_id.get(cur.parent.as_str()) {
                Some(parent) => cur = parent,
                // Orphans (parent never synced) show at the root.
                None => return true,
            }
        }
    };
    all.iter().filter(|p| is_live(p)).cloned().collect()
}

pub fn workspace_name(doc: &Doc) -> String {
    let meta = doc.get_or_insert_map(META);
    let txn = doc.transact();
    str_of(&meta, &txn, "name")
}

pub fn set_workspace_name(doc: &Doc, name: &str, origin: &str) {
    let meta = doc.get_or_insert_map(META);
    let mut txn = doc.transact_mut_with(origin);
    meta.insert(&mut txn, "name", name);
}

fn page_map<T: ReadTxn>(pages: &MapRef, txn: &T, id: &str) -> Option<MapRef> {
    match pages.get(txn, id) {
        Some(Out::YMap(m)) => Some(m),
        _ => None,
    }
}

fn siblings_order(all: &[PageMeta], parent: &str, except: &str) -> Vec<f64> {
    all.iter()
        .filter(|p| p.parent == parent && p.id != except && p.deleted == 0)
        .map(|p| p.order)
        .collect()
}

/// Order value for inserting at `index` among `orders` (sorted ascending).
pub fn order_at(orders: &[f64], index: Option<usize>) -> f64 {
    let index = index.unwrap_or(orders.len()).min(orders.len());
    match (index.checked_sub(1).map(|i| orders[i]), orders.get(index)) {
        (None, None) => 1.0,
        (None, Some(next)) => next - 1.0,
        (Some(prev), None) => prev + 1.0,
        (Some(prev), Some(next)) => (prev + next) / 2.0,
    }
}

fn insert_page(
    txn: &mut TransactionMut,
    pages: &MapRef,
    id: &str,
    title: &str,
    parent: &str,
    order: f64,
) {
    pages.insert(
        txn,
        id,
        MapPrelim::from([
            ("title", Any::from(title)),
            ("parent", Any::from(parent)),
            ("order", Any::Number(yrs::any::Number::Float(order))),
            ("icon", Any::from("")),
            ("created", Any::Number(yrs::any::Number::Int(now_ms()))),
            ("deleted", Any::Number(yrs::any::Number::Int(0))),
            ("kind", Any::from("")),
        ]),
    );
}

/// Creates a page at the end of `parent`'s children (or the root when empty).
pub fn create_page(
    doc: &Doc,
    title: &str,
    parent: &str,
    origin: &str,
) -> Result<String, TreeError> {
    let id = new_id();
    create_page_with_id(doc, &id, title, parent, origin)?;
    Ok(id)
}

pub fn create_page_with_id(
    doc: &Doc,
    id: &str,
    title: &str,
    parent: &str,
    origin: &str,
) -> Result<(), TreeError> {
    let all = all_pages(doc);
    if !parent.is_empty() && !all.iter().any(|p| p.id == parent && p.deleted == 0) {
        return Err(TreeError::NotFound(parent.to_string()));
    }
    let mut orders = siblings_order(&all, parent, id);
    orders.sort_by(f64::total_cmp);
    let order = order_at(&orders, None);
    let pages = doc.get_or_insert_map(PAGES);
    let mut txn = doc.transact_mut_with(origin);
    insert_page(&mut txn, &pages, id, title, parent, order);
    Ok(())
}

fn update_field(doc: &Doc, id: &str, origin: &str, key: &str, value: Any) -> Result<(), TreeError> {
    let pages = doc.get_or_insert_map(PAGES);
    let mut txn = doc.transact_mut_with(origin);
    let m = page_map(&pages, &txn, id).ok_or_else(|| TreeError::NotFound(id.to_string()))?;
    m.insert(&mut txn, key, value);
    Ok(())
}

pub fn rename_page(doc: &Doc, id: &str, title: &str, origin: &str) -> Result<(), TreeError> {
    update_field(doc, id, origin, "title", Any::from(title))
}

pub fn delete_page(doc: &Doc, id: &str, origin: &str) -> Result<(), TreeError> {
    update_field(
        doc,
        id,
        origin,
        "deleted",
        Any::Number(yrs::any::Number::Int(now_ms())),
    )
}

pub fn restore_page(doc: &Doc, id: &str, origin: &str) -> Result<(), TreeError> {
    update_field(
        doc,
        id,
        origin,
        "deleted",
        Any::Number(yrs::any::Number::Int(0)),
    )
}

/// Marks a page as a database (`kind` = "database") or back to an ordinary
/// page (`kind` = ""). The page body is left alone either way.
pub fn set_page_kind(doc: &Doc, id: &str, kind: &str, origin: &str) -> Result<(), TreeError> {
    update_field(doc, id, origin, "kind", Any::from(kind))
}

/// Moves `id` under `parent` ("" = root) at sibling position `index` (end when None).
pub fn move_page(
    doc: &Doc,
    id: &str,
    parent: &str,
    index: Option<usize>,
    origin: &str,
) -> Result<(), TreeError> {
    let all = all_pages(doc);
    if !all.iter().any(|p| p.id == id) {
        return Err(TreeError::NotFound(id.to_string()));
    }
    // Walk up from the new parent: reaching `id` means a cycle.
    let by_id: HashMap<&str, &PageMeta> = all.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut cur = parent;
    let mut guard = 0;
    while !cur.is_empty() && guard < all.len() + 1 {
        if cur == id {
            return Err(TreeError::Cycle);
        }
        cur = by_id.get(cur).map(|p| p.parent.as_str()).unwrap_or("");
        guard += 1;
    }
    if !parent.is_empty() && !by_id.contains_key(parent) {
        return Err(TreeError::NotFound(parent.to_string()));
    }
    let mut orders = siblings_order(&all, parent, id);
    orders.sort_by(f64::total_cmp);
    let order = order_at(&orders, index);

    let pages = doc.get_or_insert_map(PAGES);
    let mut txn = doc.transact_mut_with(origin);
    let m = page_map(&pages, &txn, id).ok_or_else(|| TreeError::NotFound(id.to_string()))?;
    m.insert(&mut txn, "parent", parent);
    m.insert(
        &mut txn,
        "order",
        Any::Number(yrs::any::Number::Float(order)),
    );
    Ok(())
}

/// Titles from the root down to `id` (inclusive).
pub fn path(pages: &[PageMeta], id: &str) -> Vec<String> {
    let by_id: HashMap<&str, &PageMeta> = pages.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut out = Vec::new();
    let mut cur = by_id.get(id).copied();
    let mut guard = 0;
    while let Some(p) = cur {
        out.push(p.title.clone());
        cur = by_id.get(p.parent.as_str()).copied();
        guard += 1;
        if guard > pages.len() {
            break;
        }
    }
    out.reverse();
    out
}

/// Live pages as an indented outline: `- Title (id)`.
pub fn outline(pages: &[PageMeta]) -> String {
    fn walk(pages: &[PageMeta], parent: &str, depth: usize, out: &mut String, ids: &HashSet<&str>) {
        for p in pages.iter().filter(|p| {
            p.parent == parent
                || (depth == 0 && !p.parent.is_empty() && !ids.contains(p.parent.as_str()))
        }) {
            let title = if p.title.is_empty() {
                "Untitled"
            } else {
                &p.title
            };
            out.push_str(&format!("{}- {} ({})\n", "  ".repeat(depth), title, p.id));
            walk(pages, &p.id, depth + 1, out, ids);
        }
    }
    let ids: HashSet<&str> = pages.iter().map(|p| p.id.as_str()).collect();
    let mut out = String::new();
    walk(pages, "", 0, &mut out, &ids);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_nest_move_delete() {
        let doc = Doc::new();
        set_workspace_name(&doc, "Notes", "t");
        assert_eq!(workspace_name(&doc), "Notes");

        create_page_with_id(&doc, "a", "A", "", "t").unwrap();
        create_page_with_id(&doc, "b", "B", "", "t").unwrap();
        create_page_with_id(&doc, "c", "C", "a", "t").unwrap();
        let mut ids: Vec<_> = live_pages(&doc).into_iter().map(|p| p.id).collect();
        ids.sort();
        assert_eq!(ids, ["a", "b", "c"]);

        // Move B before A at the root, then under C.
        move_page(&doc, "b", "", Some(0), "t").unwrap();
        let roots: Vec<_> = live_pages(&doc)
            .into_iter()
            .filter(|p| p.parent.is_empty())
            .map(|p| p.id)
            .collect();
        assert_eq!(roots, ["b", "a"]);
        move_page(&doc, "b", "c", None, "t").unwrap();
        assert_eq!(path(&all_pages(&doc), "b"), ["A", "C", "B"]);

        // A cannot move under its own grandchild.
        assert!(matches!(
            move_page(&doc, "a", "b", None, "t"),
            Err(TreeError::Cycle)
        ));

        // Deleting A hides its whole subtree; restoring brings it back.
        delete_page(&doc, "a", "t").unwrap();
        assert!(live_pages(&doc).is_empty());
        restore_page(&doc, "a", "t").unwrap();
        assert_eq!(live_pages(&doc).len(), 3);

        rename_page(&doc, "c", "See", "t").unwrap();
        assert!(outline(&live_pages(&doc)).contains("  - See (c)"));
    }

    #[test]
    fn parent_loops_surface_at_the_root() {
        let a = Doc::new();
        create_page_with_id(&a, "x", "X", "", "t").unwrap();
        create_page_with_id(&a, "y", "Y", "", "t").unwrap();
        create_page_with_id(&a, "z", "Z", "y", "t").unwrap();
        // Two replicas move X under Y and Y under X concurrently.
        let b = Doc::new();
        let sync = |from: &Doc, to: &Doc| {
            use yrs::updates::decoder::Decode;
            let u = from
                .transact()
                .encode_state_as_update_v1(&to.transact().state_vector());
            to.transact_mut()
                .apply_update(yrs::Update::decode_v1(&u).unwrap())
                .unwrap();
        };
        sync(&a, &b);
        move_page(&a, "x", "y", None, "t").unwrap();
        move_page(&b, "y", "x", None, "t").unwrap();
        sync(&a, &b);
        sync(&b, &a);
        let live: Vec<_> = live_pages(&a);
        assert_eq!(live.len(), 3, "no page may disappear: {live:?}");
        let x = live.iter().find(|p| p.id == "x").unwrap();
        assert_eq!(x.parent, "", "smallest id of the loop goes to the root");
        assert!(outline(&live).contains("- X (x)"));
    }

    #[test]
    fn create_page_returns_new_id() {
        let doc = Doc::new();
        let id = create_page(&doc, "Hello", "", "t").unwrap();
        assert_eq!(all_pages(&doc)[0].id, id);
    }

    #[test]
    fn order_between_neighbours() {
        assert_eq!(order_at(&[], None), 1.0);
        assert_eq!(order_at(&[1.0, 2.0], Some(1)), 1.5);
        assert_eq!(order_at(&[1.0, 2.0], Some(0)), 0.0);
        assert_eq!(order_at(&[1.0, 2.0], None), 3.0);
    }
}
