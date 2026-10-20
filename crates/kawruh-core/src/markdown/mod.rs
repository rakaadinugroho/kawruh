//! Markdown ↔ page document conversion.

pub mod parse;
pub mod serialize;
pub mod ydoc;

use yrs::{Doc, Transact, XmlFragmentRef};

use crate::schema::FRAGMENT;

pub use parse::markdown_to_pnode;
pub use serialize::pnode_to_markdown;

pub fn fragment(doc: &Doc) -> XmlFragmentRef {
    doc.get_or_insert_xml_fragment(FRAGMENT)
}

/// Markdown of a page document.
pub fn doc_to_markdown(doc: &Doc) -> String {
    let frag = fragment(doc);
    let txn = doc.transact();
    pnode_to_markdown(&ydoc::read_fragment(&txn, &frag))
}

/// Replaces the page content with `md`, as a single transaction tagged `origin`.
pub fn set_markdown(doc: &Doc, md: &str, origin: &str) {
    let pnode = markdown_to_pnode(md);
    let frag = fragment(doc);
    let mut txn = doc.transact_mut_with(origin);
    ydoc::replace_fragment(&mut txn, &frag, &pnode);
}

/// Appends `md` after the existing page content.
pub fn append_markdown(doc: &Doc, md: &str, origin: &str) {
    let pnode = markdown_to_pnode(md);
    let frag = fragment(doc);
    let mut txn = doc.transact_mut_with(origin);
    ydoc::append_fragment(&mut txn, &frag, &pnode);
}

/// Rewrites image sources in markdown: `f` returns the new `src`, or `None` to
/// keep it. Used to swap asset refs for files beside an exported page and back.
pub fn map_image_srcs(md: &str, mut f: impl FnMut(&str) -> Option<String>) -> String {
    fn walk(n: &mut crate::schema::PNode, f: &mut dyn FnMut(&str) -> Option<String>) {
        if let crate::schema::PNode::Element {
            name,
            attrs,
            children,
        } = n
        {
            if name == crate::schema::node::IMAGE {
                let src = match attrs.get("src") {
                    Some(yrs::Any::String(s)) => s.to_string(),
                    _ => String::new(),
                };
                if let Some(next) = f(&src) {
                    attrs.insert("src".into(), crate::schema::s(&next));
                }
            }
            for c in children {
                walk(c, f);
            }
        }
    }
    let mut doc = markdown_to_pnode(md);
    walk(&mut doc, &mut f);
    pnode_to_markdown(&doc)
}

/// Rewrites link targets (links and link cards) the same way: `f` returns the
/// new `href`, or `None` to keep it. Used to turn links between files into
/// page links on import and back on export.
pub fn map_link_hrefs(md: &str, mut f: impl FnMut(&str) -> Option<String>) -> String {
    fn href(attrs: &mut crate::schema::Attrs, f: &mut dyn FnMut(&str) -> Option<String>) {
        let old = match attrs.get("href") {
            Some(yrs::Any::String(s)) => s.to_string(),
            _ => return,
        };
        if let Some(next) = f(&old) {
            attrs.insert("href".into(), crate::schema::s(&next));
        }
    }
    fn walk(n: &mut crate::schema::PNode, f: &mut dyn FnMut(&str) -> Option<String>) {
        match n {
            crate::schema::PNode::Text { marks, .. } => {
                for m in marks
                    .iter_mut()
                    .filter(|m| m.name == crate::schema::mark::LINK)
                {
                    href(&mut m.attrs, f);
                }
            }
            crate::schema::PNode::Element {
                name,
                attrs,
                children,
            } => {
                if name == crate::schema::node::LINK_CARD {
                    href(attrs, f);
                }
                for c in children {
                    walk(c, f);
                }
            }
        }
    }
    let mut doc = markdown_to_pnode(md);
    walk(&mut doc, &mut f);
    pnode_to_markdown(&doc)
}

/// Renders markdown to sanitized HTML (raw HTML is escaped).
pub fn markdown_to_html(md: &str) -> String {
    let mut o = parse::options();
    o.render.escape = true;
    comrak::markdown_to_html(md, &o)
}

#[cfg(test)]
mod tests;
