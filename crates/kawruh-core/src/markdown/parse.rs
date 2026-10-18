//! Markdown → `PNode` (Milkdown schema), via comrak's GFM AST.

use comrak::nodes::{ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};
use yrs::Any;

use crate::schema::{Attrs, CANVAS_LANG, PMark, PNode, attrs, int, mark, node, s};

pub fn options() -> Options<'static> {
    let mut o = Options::default();
    o.extension.strikethrough = true;
    o.extension.table = true;
    o.extension.autolink = true;
    o.extension.tasklist = true;
    o.extension.footnotes = true;
    o
}

/// Parses markdown into a `doc` node that satisfies the editor schema.
pub fn markdown_to_pnode(md: &str) -> PNode {
    let arena = Arena::new();
    let root = parse_document(&arena, md, &options());
    let mut blocks = Vec::new();
    for child in root.children() {
        blocks.extend(block(child));
    }
    if blocks.is_empty() {
        blocks.push(paragraph(Vec::new()));
    }
    PNode::element(node::DOC, Attrs::new(), blocks)
}

type AstNode<'a> = comrak::Node<'a>;

fn paragraph(children: Vec<PNode>) -> PNode {
    PNode::element(node::PARAGRAPH, Attrs::new(), children)
}

fn block(n: AstNode<'_>) -> Vec<PNode> {
    let value = n.data().value.clone();
    match value {
        NodeValue::Paragraph => vec![paragraph(inlines(n))],
        NodeValue::Heading(h) => vec![PNode::element(
            node::HEADING,
            attrs([("id", s("")), ("level", int(h.level as i64))]),
            inlines(n),
        )],
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) | NodeValue::Alert(_) => {
            vec![PNode::element(
                node::BLOCKQUOTE,
                Attrs::new(),
                child_blocks(n),
            )]
        }
        NodeValue::CodeBlock(cb) => {
            let info = cb.info.split_whitespace().next().unwrap_or("").to_string();
            let mut literal = cb.literal.clone();
            if literal.ends_with('\n') {
                literal.pop();
            }
            if info == CANVAS_LANG {
                return vec![PNode::element(
                    node::CANVAS,
                    attrs([("id", s(literal.trim()))]),
                    Vec::new(),
                )];
            }
            let text = if literal.is_empty() {
                Vec::new()
            } else {
                vec![PNode::Text {
                    text: literal,
                    marks: Vec::new(),
                }]
            };
            vec![PNode::element(
                node::CODE_BLOCK,
                attrs([("language", s(&info))]),
                text,
            )]
        }
        NodeValue::ThematicBreak => vec![PNode::element(node::HR, Attrs::new(), Vec::new())],
        NodeValue::List(list) => vec![list_node(n, list.list_type, list.start, list.tight)],
        NodeValue::Table(table) => vec![table_node(n, &table.alignments)],
        NodeValue::HtmlBlock(html) => {
            let value = html.literal.trim_end_matches('\n');
            vec![paragraph(vec![PNode::element(
                node::HTML,
                attrs([("value", s(value))]),
                Vec::new(),
            )])]
        }
        NodeValue::FootnoteDefinition(def) => vec![PNode::element(
            node::FOOTNOTE_DEFINITION,
            attrs([("label", s(&def.name))]),
            non_empty(child_blocks(n)),
        )],
        NodeValue::FrontMatter(_) => Vec::new(),
        // Anything the editor has no node for degrades to its plain text.
        _ => {
            let text = collect_text(n);
            if text.is_empty() {
                Vec::new()
            } else {
                vec![paragraph(vec![plain(text)])]
            }
        }
    }
}

fn plain(text: String) -> PNode {
    PNode::Text {
        text,
        marks: Vec::new(),
    }
}

fn non_empty(mut blocks: Vec<PNode>) -> Vec<PNode> {
    if blocks.is_empty() {
        blocks.push(paragraph(Vec::new()));
    }
    blocks
}

fn child_blocks(n: AstNode<'_>) -> Vec<PNode> {
    let mut out = Vec::new();
    for c in n.children() {
        out.extend(block(c));
    }
    non_empty(out)
}

fn list_node(n: AstNode<'_>, kind: ListType, start: usize, tight: bool) -> PNode {
    let ordered = kind == ListType::Ordered;
    let mut items = Vec::new();
    for (i, item) in n.children().enumerate() {
        let checked = match &item.data().value {
            NodeValue::TaskItem(t) => Some(t.symbol.is_some()),
            _ => None,
        };
        let label = if ordered {
            format!("{}.", start + i)
        } else {
            "•".to_string()
        };
        let mut a = attrs([
            ("label", s(&label)),
            ("listType", s(if ordered { "ordered" } else { "bullet" })),
            ("spread", Any::Bool(item_spread(item))),
        ]);
        if let Some(c) = checked {
            a.insert("checked".into(), Any::Bool(c));
        }
        // list_item content is `paragraph block*`.
        let mut children = Vec::new();
        for c in item.children() {
            children.extend(block(c));
        }
        if children.first().map(|c| c.name()) != Some(node::PARAGRAPH) {
            children.insert(0, paragraph(Vec::new()));
        }
        items.push(PNode::element(node::LIST_ITEM, a, children));
    }
    if ordered {
        PNode::element(
            node::ORDERED_LIST,
            attrs([("order", int(start as i64)), ("spread", Any::Bool(!tight))]),
            items,
        )
    } else {
        PNode::element(
            node::BULLET_LIST,
            attrs([("spread", Any::Bool(!tight))]),
            items,
        )
    }
}

/// Like mdast's `listItem.spread`: a blank line separates two of its children.
fn item_spread(item: AstNode<'_>) -> bool {
    let children: Vec<_> = item.children().collect();
    children
        .windows(2)
        .any(|w| w[1].data().sourcepos.start.line > w[0].data().sourcepos.end.line + 1)
}

/// Unspecified alignment is `null` in the editor (and therefore absent in Yjs).
fn alignment(a: Option<&TableAlignment>) -> Any {
    match a {
        Some(TableAlignment::Center) => s("center"),
        Some(TableAlignment::Right) => s("right"),
        Some(TableAlignment::Left) => s("left"),
        _ => Any::Null,
    }
}

fn table_node(n: AstNode<'_>, aligns: &[TableAlignment]) -> PNode {
    let columns = aligns.len().max(1);
    let mut rows = Vec::new();
    for row in n.children() {
        let header = matches!(row.data().value, NodeValue::TableRow(true));
        let (row_name, cell_name) = if header {
            (node::TABLE_HEADER_ROW, node::TABLE_HEADER)
        } else {
            (node::TABLE_ROW, node::TABLE_CELL)
        };
        let mut cells: Vec<PNode> = row
            .children()
            .enumerate()
            .map(|(i, cell)| table_cell(cell_name, alignment(aligns.get(i)), inlines(cell)))
            .collect();
        while cells.len() < columns {
            let i = cells.len();
            cells.push(table_cell(cell_name, alignment(aligns.get(i)), Vec::new()));
        }
        rows.push(PNode::element(row_name, Attrs::new(), cells));
    }
    // table content is `table_header_row table_row+`.
    if rows.len() < 2 {
        let cells = (0..columns)
            .map(|i| table_cell(node::TABLE_CELL, alignment(aligns.get(i)), Vec::new()))
            .collect();
        rows.push(PNode::element(node::TABLE_ROW, Attrs::new(), cells));
    }
    PNode::element(node::TABLE, Attrs::new(), rows)
}

fn table_cell(name: &str, align: Any, content: Vec<PNode>) -> PNode {
    PNode::element(
        name,
        attrs([
            ("colspan", int(1)),
            ("rowspan", int(1)),
            ("alignment", align),
        ]),
        vec![paragraph(content)],
    )
}

fn collect_text(n: AstNode<'_>) -> String {
    let mut out = String::new();
    for d in n.descendants() {
        match &d.data().value {
            NodeValue::Text(t) => out.push_str(t),
            NodeValue::Code(c) => out.push_str(&c.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push('\n'),
            NodeValue::CodeBlock(cb) => out.push_str(&cb.literal),
            _ => {}
        }
    }
    out
}

/// Flattens inline content into ProseMirror text runs + inline nodes.
fn inlines(n: AstNode<'_>) -> Vec<PNode> {
    let mut out = Vec::new();
    let mut marks = Vec::new();
    for c in n.children() {
        inline(c, &mut marks, &mut out);
    }
    merge_text(out)
}

fn push_text(out: &mut Vec<PNode>, text: &str, marks: &[PMark]) {
    if text.is_empty() {
        return;
    }
    out.push(PNode::Text {
        text: text.to_string(),
        marks: sorted_marks(marks),
    });
}

/// Marks in a canonical order so equal sets compare equal.
fn sorted_marks(marks: &[PMark]) -> Vec<PMark> {
    let mut m = marks.to_vec();
    m.sort_by(|a, b| a.name.cmp(&b.name));
    m.dedup_by(|a, b| a.name == b.name);
    m
}

fn with_mark(c: AstNode<'_>, m: PMark, marks: &mut Vec<PMark>, out: &mut Vec<PNode>) {
    marks.push(m);
    for child in c.children() {
        inline(child, marks, out);
    }
    marks.pop();
}

fn inline(c: AstNode<'_>, marks: &mut Vec<PMark>, out: &mut Vec<PNode>) {
    let value = c.data().value.clone();
    match value {
        NodeValue::Text(t) => push_text(out, &t, marks),
        // Milkdown models a soft line break as an inline hardbreak.
        NodeValue::SoftBreak => out.push(PNode::element(
            node::HARDBREAK,
            attrs([("isInline", Any::Bool(true))]),
            Vec::new(),
        )),
        NodeValue::LineBreak => out.push(PNode::element(
            node::HARDBREAK,
            attrs([("isInline", Any::Bool(false))]),
            Vec::new(),
        )),
        NodeValue::Code(code) => {
            let mut m = marks.clone();
            m.push(PMark {
                name: mark::INLINE_CODE.into(),
                attrs: Attrs::new(),
            });
            push_text(out, &code.literal, &m);
        }
        NodeValue::Emph => with_mark(
            c,
            PMark {
                name: mark::EMPHASIS.into(),
                attrs: attrs([("marker", s("*"))]),
            },
            marks,
            out,
        ),
        NodeValue::Strong => with_mark(
            c,
            PMark {
                name: mark::STRONG.into(),
                attrs: attrs([("marker", s("*"))]),
            },
            marks,
            out,
        ),
        NodeValue::Strikethrough => with_mark(
            c,
            PMark {
                name: mark::STRIKE.into(),
                attrs: Attrs::new(),
            },
            marks,
            out,
        ),
        NodeValue::Link(link) => {
            let title = if link.title.is_empty() {
                Any::Null
            } else {
                s(&link.title)
            };
            with_mark(
                c,
                PMark {
                    name: mark::LINK.into(),
                    attrs: attrs([("href", s(&link.url)), ("title", title)]),
                },
                marks,
                out,
            )
        }
        NodeValue::Image(link) => out.push(PNode::element(
            node::IMAGE,
            attrs([
                ("src", s(&link.url)),
                ("alt", s(&collect_text(c))),
                ("title", s(&link.title)),
            ]),
            Vec::new(),
        )),
        NodeValue::HtmlInline(html) => out.push(PNode::element(
            node::HTML,
            attrs([("value", s(&html))]),
            Vec::new(),
        )),
        NodeValue::FootnoteReference(r) => out.push(PNode::element(
            node::FOOTNOTE_REFERENCE,
            attrs([("label", s(&r.name))]),
            Vec::new(),
        )),
        NodeValue::Escaped => {
            for child in c.children() {
                inline(child, marks, out);
            }
        }
        _ => {
            if c.first_child().is_some() {
                for child in c.children() {
                    inline(child, marks, out);
                }
            } else {
                push_text(out, &collect_text(c), marks);
            }
        }
    }
}

/// Adjacent text runs with identical marks become one, like ProseMirror does.
pub(crate) fn merge_text(nodes: Vec<PNode>) -> Vec<PNode> {
    let mut out: Vec<PNode> = Vec::with_capacity(nodes.len());
    for n in nodes {
        if let (
            Some(PNode::Text {
                text: prev,
                marks: prev_marks,
            }),
            PNode::Text { text, marks },
        ) = (out.last_mut(), &n)
            && prev_marks == marks
        {
            prev.push_str(text);
            continue;
        }
        out.push(n);
    }
    out
}
