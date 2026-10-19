//! The ProseMirror document model shared with the Milkdown editor.
//!
//! `PNode` mirrors ProseMirror's JSON shape. Markdown is parsed into it, it is
//! written to / read from the Yjs `XmlFragment` exactly the way y-prosemirror
//! does, and it is serialized back to markdown. `schema.json` is dumped from the
//! real editor (`pnpm schema:dump`) and is the contract checked by tests.

use std::collections::BTreeMap;

use yrs::Any;

pub const FRAGMENT: &str = "prosemirror";
pub const CANVAS_LANG: &str = "kawruh-canvas";
/// Link title that marks `[label](href "kawruh:card")` as a link card.
pub const LINK_CARD_PREFIX: &str = "kawruh:";
pub const LINK_CARD_VIEWS: [&str; 2] = ["card", "preview"];

pub mod node {
    pub const DOC: &str = "doc";
    pub const PARAGRAPH: &str = "paragraph";
    pub const HEADING: &str = "heading";
    pub const HARDBREAK: &str = "hardbreak";
    pub const BLOCKQUOTE: &str = "blockquote";
    pub const CODE_BLOCK: &str = "code_block";
    pub const HR: &str = "hr";
    pub const IMAGE: &str = "image";
    pub const BULLET_LIST: &str = "bullet_list";
    pub const ORDERED_LIST: &str = "ordered_list";
    pub const LIST_ITEM: &str = "list_item";
    pub const HTML: &str = "html";
    pub const TABLE: &str = "table";
    pub const TABLE_HEADER_ROW: &str = "table_header_row";
    pub const TABLE_ROW: &str = "table_row";
    pub const TABLE_HEADER: &str = "table_header";
    pub const TABLE_CELL: &str = "table_cell";
    pub const FOOTNOTE_DEFINITION: &str = "footnote_definition";
    pub const FOOTNOTE_REFERENCE: &str = "footnote_reference";
    pub const CANVAS: &str = "kawruh_canvas";
    pub const LINK_CARD: &str = "kawruh_link";
}

pub mod mark {
    pub const EMPHASIS: &str = "emphasis";
    pub const STRONG: &str = "strong";
    pub const INLINE_CODE: &str = "inlineCode";
    pub const LINK: &str = "link";
    pub const STRIKE: &str = "strike_through";
}

pub type Attrs = BTreeMap<String, Any>;

#[derive(Debug, Clone, PartialEq)]
pub struct PMark {
    pub name: String,
    pub attrs: Attrs,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PNode {
    Element {
        name: String,
        attrs: Attrs,
        children: Vec<PNode>,
    },
    Text {
        text: String,
        marks: Vec<PMark>,
    },
}

impl PNode {
    pub fn element(name: &str, attrs: Attrs, children: Vec<PNode>) -> PNode {
        PNode::Element {
            name: name.to_string(),
            attrs,
            children,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            PNode::Element { name, .. } => name,
            PNode::Text { .. } => "text",
        }
    }

    pub fn attr(&self, key: &str) -> Option<&Any> {
        match self {
            PNode::Element { attrs, .. } => attrs.get(key),
            PNode::Text { .. } => None,
        }
    }

    pub fn attr_str(&self, key: &str) -> &str {
        match self.attr(key) {
            Some(Any::String(s)) => s,
            _ => "",
        }
    }

    pub fn attr_bool(&self, key: &str) -> Option<bool> {
        match self.attr(key) {
            Some(Any::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    pub fn attr_num(&self, key: &str) -> Option<f64> {
        match self.attr(key) {
            Some(Any::Number(n)) => n.as_f64(),
            _ => None,
        }
    }

    pub fn children(&self) -> &[PNode] {
        match self {
            PNode::Element { children, .. } => children,
            PNode::Text { .. } => &[],
        }
    }

    /// Concatenated text of all descendants.
    pub fn text_content(&self) -> String {
        match self {
            PNode::Text { text, .. } => text.clone(),
            PNode::Element { children, .. } => children.iter().map(|c| c.text_content()).collect(),
        }
    }
}

pub fn s(v: &str) -> Any {
    Any::String(v.into())
}

pub fn int(v: i64) -> Any {
    Any::Number(yrs::any::Number::Int(v))
}

pub fn attrs<const N: usize>(pairs: [(&str, Any); N]) -> Attrs {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Default attribute values for every node/mark, as declared by the editor.
/// Used to fill attributes that are absent in a Yjs element (y-prosemirror
/// omits nothing but `null`s, but older or foreign writers might).
pub fn default_attrs(name: &str) -> Attrs {
    use node::*;
    match name {
        HEADING => attrs([("id", s("")), ("level", int(1))]),
        HARDBREAK => attrs([("isInline", Any::Bool(false))]),
        CODE_BLOCK => attrs([("language", s(""))]),
        IMAGE => attrs([("src", s("")), ("alt", s("")), ("title", s(""))]),
        BULLET_LIST => attrs([("spread", Any::Bool(false))]),
        ORDERED_LIST => attrs([("order", int(1)), ("spread", Any::Bool(false))]),
        LIST_ITEM => attrs([
            ("label", s("•")),
            ("listType", s("bullet")),
            ("spread", Any::Bool(true)),
        ]),
        HTML => attrs([("value", s(""))]),
        TABLE_HEADER | TABLE_CELL => attrs([
            ("colspan", int(1)),
            ("rowspan", int(1)),
            ("alignment", s("left")),
        ]),
        FOOTNOTE_DEFINITION | FOOTNOTE_REFERENCE => attrs([("label", s(""))]),
        CANVAS => attrs([("id", s(""))]),
        LINK_CARD => attrs([("href", s("")), ("label", s("")), ("view", s("card"))]),
        mark::EMPHASIS | mark::STRONG => attrs([("marker", s("*"))]),
        _ => Attrs::new(),
    }
}

#[cfg(test)]
pub(crate) mod contract {
    //! Validates `PNode` trees against the schema dumped from the editor.

    use super::*;
    use serde::Deserialize;
    use std::collections::HashMap;

    #[derive(Deserialize)]
    struct AttrSpec {
        required: bool,
        validate: Option<String>,
    }

    #[derive(Deserialize)]
    struct NodeSpec {
        group: Option<String>,
        content: Option<String>,
        attrs: HashMap<String, AttrSpec>,
    }

    #[derive(Deserialize)]
    struct MarkSpec {
        attrs: HashMap<String, AttrSpec>,
    }

    #[derive(Deserialize)]
    pub struct Contract {
        nodes: HashMap<String, NodeSpec>,
        marks: HashMap<String, MarkSpec>,
    }

    pub fn load() -> Contract {
        serde_json::from_str(include_str!("../schema.json")).expect("schema.json")
    }

    fn check_attrs(
        what: &str,
        spec: &HashMap<String, AttrSpec>,
        attrs: &Attrs,
    ) -> Result<(), String> {
        for key in attrs.keys() {
            if !spec.contains_key(key) {
                return Err(format!("{what}: unknown attr `{key}`"));
            }
        }
        for (key, a) in spec {
            let value = attrs.get(key);
            if a.required && value.is_none() {
                return Err(format!("{what}: missing required attr `{key}`"));
            }
            let (Some(value), Some(validate)) = (value, &a.validate) else {
                continue;
            };
            let ok = validate.split('|').any(|t| {
                matches!(
                    (t, value),
                    ("string", Any::String(_))
                        | ("number", Any::Number(_))
                        | ("boolean", Any::Bool(_))
                        | ("null", Any::Null)
                )
            });
            if !ok {
                return Err(format!("{what}: attr `{key}`={value:?} fails `{validate}`"));
            }
        }
        Ok(())
    }

    impl Contract {
        fn matches(&self, term: &str, child: &str) -> bool {
            term == child || self.nodes.get(child).and_then(|n| n.group.as_deref()) == Some(term)
        }

        /// Greedy matcher for the content expressions used by the schema
        /// (sequences of `name`, `name+`, `name*`, `(name)*`).
        fn content_ok(&self, expr: &str, children: &[&str]) -> bool {
            let mut i = 0;
            for raw in expr.split_whitespace() {
                let (term, q) = match raw.chars().last() {
                    Some(c @ ('+' | '*' | '?')) => (&raw[..raw.len() - 1], Some(c)),
                    _ => (raw, None),
                };
                let term = term.trim_matches(|c| c == '(' || c == ')');
                let mut n = 0;
                while i < children.len() && self.matches(term, children[i]) {
                    i += 1;
                    n += 1;
                    if q.is_none() || q == Some('?') {
                        break;
                    }
                }
                let min = matches!(q, None | Some('+')) as usize;
                if n < min {
                    return false;
                }
            }
            i == children.len()
        }

        pub fn validate(&self, node: &PNode) -> Result<(), String> {
            match node {
                PNode::Text { marks, text } => {
                    if text.is_empty() {
                        return Err("empty text node".into());
                    }
                    for m in marks {
                        let spec = self
                            .marks
                            .get(&m.name)
                            .ok_or_else(|| format!("unknown mark `{}`", m.name))?;
                        check_attrs(&m.name, &spec.attrs, &m.attrs)?;
                    }
                    Ok(())
                }
                PNode::Element {
                    name,
                    attrs,
                    children,
                } => {
                    let spec = self
                        .nodes
                        .get(name)
                        .ok_or_else(|| format!("unknown node `{name}`"))?;
                    check_attrs(name, &spec.attrs, attrs)?;
                    let kinds: Vec<&str> = children.iter().map(|c| c.name()).collect();
                    let expr = spec.content.as_deref().unwrap_or("");
                    if !self.content_ok(expr, &kinds) {
                        return Err(format!(
                            "`{name}` content {kinds:?} does not match `{expr}`"
                        ));
                    }
                    children.iter().try_for_each(|c| self.validate(c))
                }
            }
        }
    }
}
