//! `PNode` ↔ Yjs `XmlFragment`, byte-compatible with y-prosemirror:
//! elements are `XmlElement(nodeName = type name)` carrying every non-null
//! attribute; consecutive text nodes share one `XmlText` whose formatting
//! attributes are `{ markName: markAttrs }`.

use std::collections::HashMap;
use std::sync::Arc;

use yrs::types::Delta;
use yrs::types::text::YChange;
use yrs::types::xml::{XmlDeltaPrelim, XmlIn, XmlOut};
use yrs::{
    Any, In, Out, ReadTxn, Text, TransactionMut, Xml, XmlElementPrelim, XmlElementRef, XmlFragment,
    XmlFragmentRef, XmlTextRef,
};

use crate::schema::{Attrs, PMark, PNode, default_attrs, node};

fn text_delta(runs: &[(&str, &[PMark])]) -> XmlDeltaPrelim {
    let delta = runs
        .iter()
        .map(|(text, marks)| {
            let attrs: Option<Box<yrs::types::Attrs>> = if marks.is_empty() {
                None
            } else {
                Some(Box::new(
                    marks
                        .iter()
                        .map(|m| {
                            let value: HashMap<String, Any> = m
                                .attrs
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                            (Arc::<str>::from(m.name.as_str()), Any::Map(Arc::new(value)))
                        })
                        .collect(),
                ))
            };
            Delta::Inserted(In::Any(Any::String((*text).into())), attrs)
        })
        .collect();
    XmlDeltaPrelim {
        attributes: HashMap::new(),
        delta,
    }
}

/// Appends `children` to `parent`, grouping text runs like y-prosemirror.
fn write_children<P: XmlFragment>(txn: &mut TransactionMut, parent: &P, children: &[PNode]) {
    let mut i = 0;
    while i < children.len() {
        match &children[i] {
            PNode::Text { .. } => {
                let mut runs: Vec<(&str, &[PMark])> = Vec::new();
                while let Some(PNode::Text { text, marks }) = children.get(i) {
                    runs.push((text.as_str(), marks.as_slice()));
                    i += 1;
                }
                let len = parent.len(txn);
                parent.insert(txn, len, XmlIn::from(text_delta(&runs)));
            }
            PNode::Element {
                name,
                attrs,
                children: grand,
            } => {
                let len = parent.len(txn);
                let el: XmlElementRef =
                    parent.insert(txn, len, XmlElementPrelim::empty(name.as_str()));
                for (k, v) in attrs {
                    // y-prosemirror never stores null attributes.
                    if !matches!(v, Any::Null) {
                        el.insert_attribute(txn, k.as_str(), v.clone());
                    }
                }
                write_children(txn, &el, grand);
                i += 1;
            }
        }
    }
}

/// Replaces the whole fragment content with `doc`'s children.
pub fn replace_fragment(txn: &mut TransactionMut, frag: &XmlFragmentRef, doc: &PNode) {
    let len = frag.len(txn);
    if len > 0 {
        frag.remove_range(txn, 0, len);
    }
    write_children(txn, frag, doc.children());
}

/// Appends `doc`'s children after the existing fragment content.
pub fn append_fragment(txn: &mut TransactionMut, frag: &XmlFragmentRef, doc: &PNode) {
    write_children(txn, frag, doc.children());
}

fn out_to_any(out: Out) -> Option<Any> {
    match out {
        Out::Any(a) => Some(a),
        _ => None,
    }
}

fn read_element<T: ReadTxn>(txn: &T, el: &XmlElementRef) -> PNode {
    let name = el.tag().to_string();
    let mut attrs: Attrs = default_attrs(&name);
    for (k, v) in el.attributes(txn) {
        if let Some(any) = out_to_any(v) {
            attrs.insert(k.to_string(), any);
        }
    }
    let children = read_children(txn, el.children(txn).collect());
    PNode::Element {
        name,
        attrs,
        children,
    }
}

fn read_text<T: ReadTxn>(txn: &T, text: &XmlTextRef) -> Vec<PNode> {
    let mut out = Vec::new();
    for d in text.diff(txn, YChange::identity) {
        let Out::Any(Any::String(s)) = d.insert else {
            continue;
        };
        let mut marks: Vec<PMark> = d
            .attributes
            .map(|a| {
                a.iter()
                    .map(|(k, v)| {
                        // Overlapping marks are stored as `name--hash`.
                        let name = k.split("--").next().unwrap_or(k).to_string();
                        let mut attrs = default_attrs(&name);
                        if let Any::Map(m) = v {
                            for (mk, mv) in m.iter() {
                                attrs.insert(mk.clone(), mv.clone());
                            }
                        }
                        PMark { name, attrs }
                    })
                    .collect()
            })
            .unwrap_or_default();
        marks.sort_by(|a, b| a.name.cmp(&b.name));
        if !s.is_empty() {
            out.push(PNode::Text {
                text: s.to_string(),
                marks,
            });
        }
    }
    out
}

fn read_children<T: ReadTxn>(txn: &T, nodes: Vec<XmlOut>) -> Vec<PNode> {
    let mut out = Vec::new();
    for n in nodes {
        match n {
            XmlOut::Element(el) => out.push(read_element(txn, &el)),
            XmlOut::Text(t) => out.extend(read_text(txn, &t)),
            XmlOut::Fragment(f) => out.extend(read_children(txn, f.children(txn).collect())),
        }
    }
    crate::markdown::parse::merge_text(out)
}

/// Reads the fragment as a `doc` node.
pub fn read_fragment<T: ReadTxn>(txn: &T, frag: &XmlFragmentRef) -> PNode {
    let children = read_children(txn, frag.children(txn).collect());
    PNode::element(node::DOC, Attrs::new(), children)
}
