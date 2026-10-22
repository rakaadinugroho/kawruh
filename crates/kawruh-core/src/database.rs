//! Database pages: a table of rows with typed properties.
//!
//! Layout (mirrored by `app/src/lib/database.ts`):
//! - In the workspace document, the page's entry carries `kind: "database"`.
//! - In the page's own document:
//!   `schema: Map{ value: <json> }` (the list of properties),
//!   `view: Map{ value: "table" | "board" }` and
//!   `rows: Map<rowId, Map<propertyId, value>>`.
//!
//! Rows are not pages: they never appear in the page tree, and a property
//! value is stored under its own key, so two people editing different cells of
//! the same row never overwrite each other.
//!
//! A database is written to markdown as a `kawruh-database` code fence holding
//! its JSON, so it survives an export and an import.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use yrs::{Any, Doc, Map, MapRef, Out, ReadTxn, Transact};

/// The map of rows in a database page.
pub const ROWS: &str = "rows";
/// The JSON list of properties.
pub const SCHEMA: &str = "schema";
/// Which view the page opens in: "table" or "board".
pub const VIEW: &str = "view";
/// The page kind of a database page.
pub const KIND_DATABASE: &str = "database";
/// The fenced-code language a database is written as.
pub const DATABASE_LANG: &str = "kawruh-database";
/// The key a row's position is kept under; property ids never start with `_`.
pub const ORDER_KEY: &str = "__order";

/// The property kinds a column can have.
pub const KINDS: [&str; 9] = [
    "title",
    "text",
    "number",
    "select",
    "multi_select",
    "date",
    "checkbox",
    "person",
    "url",
];

/// One column of a database.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Property {
    pub id: String,
    pub name: String,
    /// The kind of value the column holds (see `KINDS`).
    #[serde(rename = "type")]
    pub kind: String,
    /// The choices of a `select` or `multi_select` column.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

impl Property {
    pub fn new(id: &str, name: &str, kind: &str) -> Property {
        Property {
            id: id.to_string(),
            name: name.to_string(),
            kind: kind.to_string(),
            options: Vec::new(),
        }
    }

    /// Whether the kind is one the app knows.
    pub fn is_known(&self) -> bool {
        KINDS.contains(&self.kind.as_str())
    }
}

/// A value in a cell. Every kind maps onto one of these four.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    Number(f64),
    Bool(bool),
    List(Vec<String>),
}

impl Cell {
    /// The value as plain text, for tables, search and exports.
    pub fn text(&self) -> String {
        match self {
            Cell::Text(s) => s.clone(),
            Cell::Number(n) => number_text(*n),
            Cell::Bool(b) => {
                if *b {
                    "yes".into()
                } else {
                    String::new()
                }
            }
            Cell::List(items) => items.join(", "),
        }
    }

    fn from_any(value: &Any) -> Cell {
        match value {
            Any::String(s) => Cell::Text(s.to_string()),
            Any::Number(n) => Cell::Number(n.as_f64().unwrap_or(0.0)),
            Any::Bool(b) => Cell::Bool(*b),
            Any::Array(items) => Cell::List(
                items
                    .iter()
                    .filter_map(|v| match v {
                        Any::String(s) => Some(s.to_string()),
                        _ => None,
                    })
                    .collect(),
            ),
            _ => Cell::Text(String::new()),
        }
    }

    fn to_any(&self) -> Any {
        match self {
            Cell::Text(s) => Any::from(s.clone()),
            Cell::Number(n) => Any::Number(yrs::any::Number::Float(*n)),
            Cell::Bool(b) => Any::Bool(*b),
            Cell::List(items) => {
                let list: Vec<Any> = items.iter().map(|s| Any::from(s.clone())).collect();
                Any::Array(list.into())
            }
        }
    }
}

/// A number as plain text: whole numbers lose their decimals.
fn number_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// One row of a database.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    /// The cells, by property id.
    pub cells: BTreeMap<String, Cell>,
    /// Where the row sits among the others.
    pub order: f64,
}

impl Row {
    /// The value of one property, or an empty cell.
    pub fn get(&self, property: &str) -> Cell {
        self.cells
            .get(property)
            .cloned()
            .unwrap_or(Cell::Text(String::new()))
    }

    /// The row's title: the value of the first `title` column.
    pub fn title(&self, schema: &[Property]) -> String {
        match schema.iter().find(|p| p.kind == "title") {
            Some(p) => self.get(&p.id).text(),
            None => String::new(),
        }
    }
}

// ---- the schema ------------------------------------------------------------

/// The properties of a database page, in order.
pub fn schema(doc: &Doc) -> Vec<Property> {
    let txn = doc.transact();
    let Some(Out::YMap(map)) = txn.get(SCHEMA) else {
        return Vec::new();
    };
    let Some(Out::Any(Any::String(raw))) = map.get(&txn, "value") else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Writes the properties of a database page.
pub fn set_schema(doc: &Doc, properties: &[Property], origin: &str) {
    let json = serde_json::to_string(properties).unwrap_or_else(|_| "[]".into());
    let map = doc.get_or_insert_map(SCHEMA);
    let mut txn = doc.transact_mut_with(origin);
    map.insert(&mut txn, "value", json);
}

/// The view a database opens in ("table" or "board").
pub fn view(doc: &Doc) -> String {
    let txn = doc.transact();
    let Some(Out::YMap(map)) = txn.get(VIEW) else {
        return "table".to_string();
    };
    match map.get(&txn, "value") {
        Some(Out::Any(Any::String(s))) => s.to_string(),
        _ => "table".to_string(),
    }
}

/// Sets the view a database opens in.
pub fn set_view(doc: &Doc, view: &str, origin: &str) {
    let map = doc.get_or_insert_map(VIEW);
    let mut txn = doc.transact_mut_with(origin);
    map.insert(&mut txn, "value", view.to_string());
}

/// A new database's columns: a name, a status and a date.
pub fn default_schema() -> Vec<Property> {
    vec![
        Property::new("name", "Name", "title"),
        Property {
            id: "status".into(),
            name: "Status".into(),
            kind: "select".into(),
            options: vec!["Todo".into(), "Doing".into(), "Done".into()],
        },
        Property::new("due", "Due", "date"),
    ]
}

// ---- the rows --------------------------------------------------------------

/// An empty map, the value a fresh row is written as.
fn empty_map() -> yrs::MapPrelim {
    yrs::MapPrelim::from([] as [(&str, yrs::In); 0])
}

fn row_map<'a, T: ReadTxn>(rows: &'a MapRef, txn: &'a T, id: &str) -> Option<MapRef> {
    match rows.get(txn, id) {
        Some(Out::YMap(m)) => Some(m),
        _ => None,
    }
}

/// The rows of a database page, in the order they were created.
pub fn rows(doc: &Doc) -> Vec<Row> {
    let txn = doc.transact();
    let Some(Out::YMap(rows_map)) = txn.get(ROWS) else {
        return Vec::new();
    };
    let mut out: Vec<Row> = rows_map
        .iter(&txn)
        .filter_map(|(id, v)| match v {
            Out::YMap(m) => {
                let mut cells = BTreeMap::new();
                let mut order = 0.0;
                m.iter(&txn).for_each(|(key, value)| {
                    if key == ORDER_KEY {
                        if let Out::Any(Any::Number(n)) = value {
                            order = n.as_f64().unwrap_or(0.0);
                        }
                        return;
                    }
                    if let Out::Any(any) = value {
                        cells.insert(key.to_string(), Cell::from_any(&any));
                    }
                });
                Some(Row {
                    id: id.to_string(),
                    cells,
                    order,
                })
            }
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.order.total_cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    out
}

/// Writes a row's cells. Cells that are empty are left out.
pub fn write_row(doc: &Doc, id: &str, cells: &BTreeMap<String, Cell>, origin: &str) {
    let rows_map = doc.get_or_insert_map(ROWS);
    let mut txn = doc.transact_mut_with(origin);
    let row = match row_map(&rows_map, &txn, id) {
        Some(m) => m,
        None => {
            rows_map.insert(&mut txn, id, empty_map());
            match row_map(&rows_map, &txn, id) {
                Some(m) => m,
                None => return,
            }
        }
    };
    for (key, cell) in cells {
        row.insert(&mut txn, key.as_str(), cell.to_any());
    }
}

/// Writes one cell of a row, leaving the rest of the row alone.
pub fn set_cell(doc: &Doc, row_id: &str, property: &str, value: Cell, origin: &str) {
    let mut cells = BTreeMap::new();
    cells.insert(property.to_string(), value);
    write_row(doc, row_id, &cells, origin);
}

/// Removes a row.
pub fn remove_row(doc: &Doc, id: &str, origin: &str) {
    let rows_map = doc.get_or_insert_map(ROWS);
    let mut txn = doc.transact_mut_with(origin);
    rows_map.remove(&mut txn, id);
}

/// Adds an empty row at the end and returns its id.
pub fn add_row(doc: &Doc, origin: &str) -> String {
    let id = crate::workspace::new_id();
    let order = rows(doc).last().map(|r| r.order + 1.0).unwrap_or(1.0);
    let rows_map = doc.get_or_insert_map(ROWS);
    let mut txn = doc.transact_mut_with(origin);
    let row = rows_map.insert(&mut txn, id.as_str(), empty_map());
    row.insert(
        &mut txn,
        ORDER_KEY,
        Any::Number(yrs::any::Number::Float(order)),
    );
    id
}

// ---- reading and writing the whole thing ----------------------------------

/// The database as one JSON value: the columns and the rows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Database {
    pub columns: Vec<Property>,
    pub rows: Vec<RowValue>,
}

/// One row as it is written to JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RowValue {
    pub id: String,
    pub cells: BTreeMap<String, serde_json::Value>,
}

/// The database of a page, if it has one.
pub fn database(doc: &Doc) -> Option<Database> {
    let columns = schema(doc);
    if columns.is_empty() {
        return None;
    }
    let rows = rows(doc)
        .into_iter()
        .map(|r| RowValue {
            id: r.id,
            cells: r
                .cells
                .into_iter()
                .map(|(k, v)| (k, cell_to_json(&v)))
                .collect(),
        })
        .collect();
    Some(Database { columns, rows })
}

fn cell_to_json(cell: &Cell) -> serde_json::Value {
    match cell {
        Cell::Text(s) => serde_json::Value::String(s.clone()),
        Cell::Number(n) => serde_json::json!(n),
        Cell::Bool(b) => serde_json::Value::Bool(*b),
        Cell::List(items) => serde_json::json!(items),
    }
}

/// The database as the JSON block a page carries.
pub fn to_json(doc: &Doc) -> Option<String> {
    let db = database(doc)?;
    serde_json::to_string(&db).ok()
}

/// The database of a page, read from a JSON block.
pub fn from_json(raw: &str) -> Option<Database> {
    serde_json::from_str(raw).ok()
}

/// The database as a `kawruh-database` code fence, the way it is written to
/// markdown.
pub fn to_block(doc: &Doc) -> Option<String> {
    let json = to_json(doc)?;
    Some(format!("```{DATABASE_LANG}\n{json}\n```"))
}

/// The database of a page, read from a `kawruh-database` code fence.
pub fn from_block(markdown: &str) -> Option<Database> {
    let fence = format!("```{DATABASE_LANG}");
    let start = markdown.find(&fence)?;
    let after = &markdown[start + fence.len()..];
    let end = after.find("```")?;
    from_json(after[..end].trim())
}

/// Takes the `kawruh-database` block out of a page's markdown: the rest of the
/// page, and the database it carried.
pub fn split_block(markdown: &str) -> Option<(String, Database)> {
    let fence = format!("```{DATABASE_LANG}");
    let start = markdown.find(&fence)?;
    let after = &markdown[start + fence.len()..];
    let end = after.find("```")?;
    let db = from_json(after[..end].trim())?;
    let rest = format!("{}{}", &markdown[..start], &after[end + "```".len()..]);
    Some((rest, db))
}

/// A page's markdown: its body, plus the database it carries (as the
/// `kawruh-database` block that keeps it a database after an import).
pub fn page_markdown(doc: &Doc, is_database: bool) -> String {
    let body = crate::markdown::doc_to_markdown(doc);
    if !is_database {
        return body;
    }
    match to_block(doc) {
        Some(block) if body.trim().is_empty() => block,
        Some(block) => format!("{body}\n\n{block}"),
        None => body,
    }
}

/// The database as a markdown table, for reading and sharing.
pub fn to_markdown(doc: &Doc) -> Option<String> {
    let db = database(doc)?;
    Some(markdown_of(&db))
}

/// The database as a markdown table.
pub fn markdown_of(db: &Database) -> String {
    if db.columns.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let header: Vec<String> = db.columns.iter().map(|c| escape(&c.name)).collect();
    out.push_str(&format!("| {} |\n", header.join(" | ")));
    out.push_str(&format!(
        "| {} |\n",
        db.columns
            .iter()
            .map(|_| "---")
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    for row in &db.rows {
        let cells: Vec<String> = db
            .columns
            .iter()
            .map(|c| {
                let value = row.cells.get(&c.id).map(json_to_text).unwrap_or_default();
                escape(&value)
            })
            .collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out.trim_end().to_string()
}

/// A cell value as plain text.
pub fn json_to_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => {
            if *b {
                "yes".into()
            } else {
                String::new()
            }
        }
        serde_json::Value::Number(n) => n.as_f64().map(number_text).unwrap_or_default(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(json_to_text)
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

/// Keeps a `|` or a newline from breaking the table.
fn escape(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// Writes a whole database into a page document.
pub fn write_database(doc: &Doc, db: &Database, origin: &str) {
    set_schema(doc, &db.columns, origin);
    let rows_map = doc.get_or_insert_map(ROWS);
    let mut txn = doc.transact_mut_with(origin);
    for (index, row) in db.rows.iter().enumerate() {
        let row_ref = match row_map(&rows_map, &txn, &row.id) {
            Some(m) => m,
            None => {
                rows_map.insert(&mut txn, row.id.as_str(), empty_map());
                match row_map(&rows_map, &txn, &row.id) {
                    Some(m) => m,
                    None => continue,
                }
            }
        };
        row_ref.insert(
            &mut txn,
            ORDER_KEY,
            Any::Number(yrs::any::Number::Float(index as f64 + 1.0)),
        );
        for (key, value) in &row.cells {
            row_ref.insert(&mut txn, key.as_str(), json_to_cell(value).to_any());
        }
    }
}

fn json_to_cell(value: &serde_json::Value) -> Cell {
    match value {
        serde_json::Value::String(s) => Cell::Text(s.clone()),
        serde_json::Value::Bool(b) => Cell::Bool(*b),
        serde_json::Value::Number(n) => Cell::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::Array(items) => Cell::List(items.iter().map(json_to_text).collect()),
        _ => Cell::Text(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(rows: &[(&str, &str, &str)]) -> Doc {
        let doc = Doc::new();
        set_schema(&doc, &default_schema(), "t");
        for (id, name, status) in rows {
            let mut cells = BTreeMap::new();
            cells.insert("name".to_string(), Cell::Text(name.to_string()));
            cells.insert("status".to_string(), Cell::Text(status.to_string()));
            write_row(&doc, id, &cells, "t");
        }
        doc
    }

    #[test]
    fn the_schema_round_trips() {
        let doc = Doc::new();
        let props = default_schema();
        set_schema(&doc, &props, "t");
        assert_eq!(schema(&doc), props);
        // The status column knows its choices.
        assert_eq!(schema(&doc)[1].options, ["Todo", "Doing", "Done"]);
    }

    #[test]
    fn rows_are_read_back() {
        let doc = doc_with(&[("r1", "Write the docs", "Doing")]);
        let rows = rows(&doc);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "r1");
        assert_eq!(rows[0].get("name").text(), "Write the docs");
        assert_eq!(rows[0].get("status").text(), "Doing");
        // An empty cell is empty, not missing.
        assert_eq!(rows[0].get("due").text(), "");
    }

    #[test]
    fn a_database_survives_a_json_round_trip() {
        let doc = doc_with(&[("r1", "Write the docs", "Doing"), ("r2", "Ship it", "Todo")]);
        let json = to_json(&doc).unwrap();
        let db = from_json(&json).unwrap();
        assert_eq!(db.columns.len(), 3);
        assert_eq!(db.rows.len(), 2);
        assert_eq!(
            db.rows[0].cells["name"],
            serde_json::json!("Write the docs")
        );
    }

    #[test]
    fn a_database_reads_as_a_markdown_table() {
        let doc = doc_with(&[("r1", "Write the docs", "Doing")]);
        let md = to_markdown(&doc).unwrap();
        assert!(md.contains("| Name | Status | Due |"), "{md}");
        assert!(md.contains("| Write the docs | Doing |  |"), "{md}");
    }

    #[test]
    fn a_pipe_in_a_value_does_not_break_the_table() {
        let doc = Doc::new();
        set_schema(&doc, &default_schema(), "t");
        let mut cells = BTreeMap::new();
        cells.insert("name".to_string(), Cell::Text("a | b".into()));
        write_row(&doc, "r1", &cells, "t");
        let md = to_markdown(&doc).unwrap();
        assert!(md.contains("a \\| b"), "{md}");
    }

    #[test]
    fn a_removed_row_is_gone() {
        let doc = doc_with(&[("r1", "Write the docs", "Doing")]);
        remove_row(&doc, "r1", "t");
        assert!(rows(&doc).is_empty());
    }

    #[test]
    fn the_view_is_remembered() {
        let doc = Doc::new();
        assert_eq!(view(&doc), "table");
        set_view(&doc, "board", "t");
        assert_eq!(view(&doc), "board");
    }

    #[test]
    fn a_new_row_lands_at_the_end() {
        let doc = Doc::new();
        set_schema(&doc, &default_schema(), "t");
        let a = add_row(&doc, "t");
        let b = add_row(&doc, "t");
        let rows = rows(&doc);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, a);
        assert_eq!(rows[1].id, b);
    }

    #[test]
    fn one_cell_is_written_without_touching_the_others() {
        let doc = doc_with(&[("r1", "Write the docs", "Doing")]);
        set_cell(
            &doc,
            "r1",
            "name",
            Cell::Text("Write the docs, again".into()),
            "t",
        );
        let rows = rows(&doc);
        assert_eq!(rows[0].get("name").text(), "Write the docs, again");
        assert_eq!(rows[0].get("status").text(), "Doing");
    }

    #[test]
    fn a_database_survives_a_block_round_trip() {
        let doc = doc_with(&[("r1", "Write the docs", "Doing")]);
        let block = to_block(&doc).unwrap();
        assert!(block.starts_with("```kawruh-database"), "{block}");
        let db = from_block(&block).unwrap();
        assert_eq!(db.columns.len(), 3);
        assert_eq!(
            db.rows[0].cells["name"],
            serde_json::json!("Write the docs")
        );
        // A block inside a longer page is found too.
        let page = format!("Some text\n\n{block}\n\nMore text\n");
        assert!(from_block(&page).is_some());
    }

    #[test]
    fn the_block_is_taken_out_of_the_page() {
        let doc = doc_with(&[("r1", "Write the docs", "Doing")]);
        let block = to_block(&doc).unwrap();
        let page = format!("Some text\n\n{block}\n\nMore text\n");
        let (rest, db) = split_block(&page).unwrap();
        assert!(!rest.contains("kawruh-database"), "{rest}");
        assert!(rest.contains("Some text"));
        assert!(rest.contains("More text"));
        assert_eq!(db.rows.len(), 1);
    }

    #[test]
    fn a_database_can_be_written_whole() {
        let doc = Doc::new();
        let db = Database {
            columns: default_schema(),
            rows: vec![RowValue {
                id: "r1".into(),
                cells: BTreeMap::from([("name".into(), serde_json::json!("Ship it"))]),
            }],
        };
        write_database(&doc, &db, "t");
        assert_eq!(rows(&doc).len(), 1);
        assert_eq!(rows(&doc)[0].get("name").text(), "Ship it");
    }
}
