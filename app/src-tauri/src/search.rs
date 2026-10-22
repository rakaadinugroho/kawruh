//! Full-text search and backlinks over page bodies. Page text is cached in
//! memory and re-read only for pages that changed since the last query, so a
//! search touches the documents once and then stays cheap.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use kawruh_core::markdown;
use kawruh_core::schema::{PNode, node};
use kawruh_core::workspace::{self, PageMeta};
use serde::Serialize;

use crate::error::Result;
use crate::hub::DocHub;

/// Markdown form of a link to another page.
pub const PAGE_LINK: &str = "kawruh://page/";

#[derive(Default)]
struct Cached {
    /// Lowercased plain text, for matching.
    lower: String,
    /// Plain text, for snippets.
    plain: String,
    /// Markdown, for finding page links.
    markdown: String,
}

#[derive(Default)]
pub struct SearchIndex {
    cache: Mutex<HashMap<String, Cached>>,
    dirty: Mutex<HashSet<String>>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub snippet: String,
    pub in_title: bool,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PageRef {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub title: String,
    pub icon: String,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
}

/// Live pages and the links between them, for the graph view.
#[derive(Debug, Serialize, PartialEq)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

impl SearchIndex {
    /// A document changed; re-read it on the next query.
    pub fn mark(&self, doc_id: &str) {
        self.dirty.lock().unwrap().insert(doc_id.to_string());
    }

    fn refresh(&self, hub: &DocHub, workspace_id: &str, pages: &[PageMeta]) -> Result<()> {
        let stale: Vec<&PageMeta> = {
            let cache = self.cache.lock().unwrap();
            let dirty = self.dirty.lock().unwrap();
            pages
                .iter()
                .filter(|p| dirty.contains(&p.id) || !cache.contains_key(&p.id))
                .collect()
        };
        for page in stale {
            let doc = hub.get_or_load(&page.id, workspace_id)?;
            let markdown = markdown::doc_to_markdown(&doc);
            let plain = plain_text(&markdown);
            let entry = Cached {
                lower: plain.to_lowercase(),
                plain,
                markdown,
            };
            self.dirty.lock().unwrap().remove(&page.id);
            self.cache.lock().unwrap().insert(page.id.clone(), entry);
        }
        Ok(())
    }

    /// Pages whose title or body contains every word of `query`: title matches
    /// first, then by how often the words occur.
    pub fn search(
        &self,
        hub: &DocHub,
        workspace_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let ws = hub.get_or_load(workspace_id, workspace_id)?;
        let pages = workspace::live_pages(&ws);
        self.refresh(hub, workspace_id, &pages)?;
        let cache = self.cache.lock().unwrap();
        let mut hits: Vec<(SearchHit, usize)> = Vec::new();
        for page in &pages {
            let Some(text) = cache.get(&page.id) else {
                continue;
            };
            let title = page.title.to_lowercase();
            if !terms
                .iter()
                .all(|t| title.contains(t) || text.lower.contains(t))
            {
                continue;
            }
            let in_title = terms.iter().all(|t| title.contains(t));
            let count = terms.iter().map(|t| text.lower.matches(t).count()).sum();
            hits.push((
                SearchHit {
                    id: page.id.clone(),
                    title: page.title.clone(),
                    snippet: snippet(&text.plain, &text.lower, &terms),
                    in_title,
                },
                count,
            ));
        }
        hits.sort_by(|a, b| b.0.in_title.cmp(&a.0.in_title).then(b.1.cmp(&a.1)));
        Ok(hits.into_iter().take(limit).map(|(h, _)| h).collect())
    }

    /// Live pages that link to `page_id`.
    pub fn backlinks(
        &self,
        hub: &DocHub,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<Vec<PageRef>> {
        let ws = hub.get_or_load(workspace_id, workspace_id)?;
        let pages = workspace::live_pages(&ws);
        self.refresh(hub, workspace_id, &pages)?;
        let needle = format!("{PAGE_LINK}{page_id}");
        let cache = self.cache.lock().unwrap();
        Ok(pages
            .iter()
            .filter(|p| p.id != page_id)
            .filter(|p| {
                cache
                    .get(&p.id)
                    .is_some_and(|c| links_to(&c.markdown, &needle))
            })
            .map(|p| PageRef {
                id: p.id.clone(),
                title: p.title.clone(),
            })
            .collect())
    }

    /// Every live page, and an edge for each page that links to another.
    pub fn graph(&self, hub: &DocHub, workspace_id: &str) -> Result<Graph> {
        let ws = hub.get_or_load(workspace_id, workspace_id)?;
        let pages = workspace::live_pages(&ws);
        self.refresh(hub, workspace_id, &pages)?;
        let cache = self.cache.lock().unwrap();
        Ok(link_graph(&pages, |id| {
            cache.get(id).map(|c| c.markdown.as_str())
        }))
    }
}

/// Nodes for `pages` and one edge per distinct link from one of them to
/// another (links to itself or to pages not in `pages` are left out).
fn link_graph<'a>(pages: &[PageMeta], markdown_of: impl Fn(&str) -> Option<&'a str>) -> Graph {
    let live: HashSet<&str> = pages.iter().map(|p| p.id.as_str()).collect();
    let mut edges = Vec::new();
    for page in pages {
        let Some(markdown) = markdown_of(&page.id) else {
            continue;
        };
        let mut seen = HashSet::new();
        for to in linked_ids(markdown) {
            if to != page.id && live.contains(to) && seen.insert(to) {
                edges.push(GraphEdge {
                    from: page.id.clone(),
                    to: to.to_string(),
                });
            }
        }
    }
    let nodes = pages
        .iter()
        .map(|p| GraphNode {
            id: p.id.clone(),
            title: p.title.clone(),
            icon: p.icon.clone(),
        })
        .collect();
    Graph { nodes, edges }
}

/// The page id of every page link in `markdown`, in order: the letters and
/// digits after the prefix, the same boundary `links_to` uses.
fn linked_ids(markdown: &str) -> impl Iterator<Item = &str> {
    markdown.match_indices(PAGE_LINK).filter_map(|(i, _)| {
        let rest = &markdown[i + PAGE_LINK.len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric())
            .unwrap_or(rest.len());
        (end > 0).then(|| &rest[..end])
    })
}

/// True when `markdown` has a link to exactly `target` (not to a page whose id
/// merely starts with it).
fn links_to(markdown: &str, target: &str) -> bool {
    markdown.match_indices(target).any(|(i, _)| {
        let rest = &markdown[i + target.len()..];
        !rest.starts_with(|c: char| c.is_ascii_alphanumeric())
    })
}

/// The words of a page without markdown syntax, one block per line.
pub fn plain_text(md: &str) -> String {
    fn collect(n: &PNode, out: &mut String) {
        match n {
            PNode::Text { text, .. } => out.push_str(text),
            PNode::Element { name, children, .. } => match name.as_str() {
                node::HARDBREAK => out.push('\n'),
                node::IMAGE | node::HTML | node::FOOTNOTE_REFERENCE => {}
                node::LINK_CARD => {
                    out.push_str(n.attr_str("label"));
                    out.push('\n');
                }
                _ => {
                    for c in children {
                        collect(c, out);
                    }
                    out.push('\n');
                }
            },
        }
    }
    let mut out = String::new();
    collect(&markdown::markdown_to_pnode(md), &mut out);
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// About 140 characters of `plain` around the first matching word.
fn snippet(plain: &str, lower: &str, terms: &[String]) -> String {
    const SIDE: usize = 60;
    let at = terms.iter().filter_map(|t| lower.find(t.as_str())).min();
    let Some(at) = at else {
        return plain
            .chars()
            .take(SIDE * 2)
            .collect::<String>()
            .replace('\n', " ");
    };
    // Lowercasing can change byte offsets for some scripts; work in chars.
    let char_at = lower[..at].chars().count();
    let chars: Vec<char> = plain.chars().collect();
    let start = char_at.saturating_sub(SIDE);
    let end = (char_at + SIDE * 2).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if start > 0 {
        out.insert(0, '…');
    }
    if end < chars.len() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_drops_markdown_syntax() {
        let md = "# Plan\n\nShip **v1** with [docs](https://e.com/x).\n\n- [ ] one\n- two\n";
        assert_eq!(plain_text(md), "Plan\nShip v1 with docs.\none\ntwo");
    }

    #[test]
    fn snippets_center_on_the_first_match() {
        let plain = format!("{} needle {}", "a ".repeat(80), "b ".repeat(80));
        let s = snippet(&plain, &plain.to_lowercase(), &["needle".into()]);
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.contains("needle"));
        assert!(s.chars().count() < 200);
    }

    #[test]
    fn page_links_match_whole_ids() {
        let md = "See [A](kawruh://page/abc) and [B](kawruh://page/abcd).";
        assert!(links_to(md, "kawruh://page/abc"));
        assert!(links_to(md, "kawruh://page/abcd"));
        assert!(!links_to("[A](kawruh://page/abcd)", "kawruh://page/abc"));
    }

    fn meta(id: &str) -> PageMeta {
        PageMeta {
            id: id.into(),
            title: id.to_uppercase(),
            parent: String::new(),
            order: 0.0,
            icon: String::new(),
            created: 0,
            deleted: 0,
        }
    }

    fn edges(g: &Graph) -> Vec<(&str, &str)> {
        g.edges
            .iter()
            .map(|e| (e.from.as_str(), e.to.as_str()))
            .collect()
    }

    #[test]
    fn linked_ids_stop_at_the_id_boundary() {
        let md = "[A](kawruh://page/abc) [B](kawruh://page/abcd#x) kawruh://page/ [x](https://e.com)";
        assert_eq!(linked_ids(md).collect::<Vec<_>>(), ["abc", "abcd"]);
    }

    #[test]
    fn graph_edges_are_distinct_links_between_live_pages() {
        // "gone" is not live (deleted); "abcd" is live but "abc" only prefixes it.
        let pages = [meta("abc"), meta("abcd"), meta("c")];
        let md: HashMap<&str, &str> = HashMap::from([
            (
                "abc",
                "[D](kawruh://page/abcd) again [D](kawruh://page/abcd) [C](kawruh://page/c)",
            ),
            ("abcd", "[me](kawruh://page/abcd) [gone](kawruh://page/gone)"),
            ("c", "[A](kawruh://page/abc), [Ax](kawruh://page/abcx)"),
        ]);
        let g = link_graph(&pages, |id| md.get(id).copied());
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.nodes[0].title, "ABC");
        assert_eq!(edges(&g), [("abc", "abcd"), ("abc", "c"), ("c", "abc")]);
    }

    #[test]
    fn graph_of_pages_without_text_has_no_edges() {
        let g = link_graph(&[meta("a")], |_| None);
        assert_eq!(g.nodes.len(), 1);
        assert!(g.edges.is_empty());
    }
}
