use std::fs;
use std::path::PathBuf;

use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use super::*;
use crate::schema::contract;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixtures() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = fs::read_dir(fixtures_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "md"))
        .map(|p| {
            (
                p.file_stem().unwrap().to_string_lossy().to_string(),
                fs::read_to_string(&p).unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

fn doc_from_md(md: &str) -> Doc {
    let doc = Doc::new();
    set_markdown(&doc, md, "test");
    doc
}

fn read(doc: &Doc) -> crate::schema::PNode {
    let frag = fragment(doc);
    let txn = doc.transact();
    ydoc::read_fragment(&txn, &frag)
}

/// What a tree looks like after a trip through Yjs: y-prosemirror never stores
/// `null` element attributes, so they read back as the schema default.
fn like_yjs(n: &crate::schema::PNode) -> crate::schema::PNode {
    use crate::schema::{PNode, default_attrs};
    match n {
        PNode::Text { .. } => n.clone(),
        PNode::Element {
            name,
            attrs,
            children,
        } => {
            let defaults = default_attrs(name);
            let attrs = attrs
                .iter()
                .filter_map(|(k, v)| match v {
                    yrs::Any::Null => defaults.get(k).map(|d| (k.clone(), d.clone())),
                    _ => Some((k.clone(), v.clone())),
                })
                .collect();
            PNode::Element {
                name: name.clone(),
                attrs,
                children: children.iter().map(like_yjs).collect(),
            }
        }
    }
}

#[test]
fn fixtures_satisfy_schema_contract() {
    let c = contract::load();
    for (name, md) in fixtures() {
        let p = markdown_to_pnode(&md);
        c.validate(&p)
            .unwrap_or_else(|e| panic!("{name}: {e}\n{p:#?}"));
    }
}

#[test]
fn yjs_roundtrip_is_exact() {
    for (name, md) in fixtures() {
        let p = like_yjs(&markdown_to_pnode(&md));
        let doc = doc_from_md(&md);
        assert_eq!(read(&doc), p, "{name}: Yjs round trip changed the tree");

        // Survives encoding to a v1 update and back into a fresh doc.
        let update = doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let fresh = Doc::new();
        fresh
            .transact_mut()
            .apply_update(Update::decode_v1(&update).unwrap())
            .unwrap();
        assert_eq!(read(&fresh), p, "{name}: update encoding changed the tree");
    }
}

#[test]
fn markdown_roundtrip_is_stable() {
    for (name, md) in fixtures() {
        let p1 = markdown_to_pnode(&md);
        let md2 = pnode_to_markdown(&p1);
        let p2 = markdown_to_pnode(&md2);
        assert_eq!(
            p1, p2,
            "{name}: re-parsing serialized markdown changed the tree\n{md2}"
        );
        let md3 = pnode_to_markdown(&p2);
        assert_eq!(md2, md3, "{name}: serializer is not idempotent");
    }
}

#[test]
fn append_keeps_existing_content() {
    let doc = doc_from_md("# Title\n\nfirst");
    append_markdown(&doc, "second", "test");
    assert_eq!(doc_to_markdown(&doc), "# Title\n\nfirst\n\nsecond\n");
}

#[test]
fn empty_markdown_yields_one_empty_paragraph() {
    let p = markdown_to_pnode("");
    assert_eq!(p.children().len(), 1);
    contract::load().validate(&p).unwrap();
}

#[test]
fn canvas_and_mermaid_are_lossless() {
    let md = "```mermaid\ngraph TD\n  A --> B\n```\n\n```kawruh-canvas\nabc\n```\n";
    let doc = doc_from_md(md);
    assert_eq!(doc_to_markdown(&doc), md);
    let p = read(&doc);
    assert_eq!(p.children()[0].attr_str("language"), "mermaid");
    assert_eq!(p.children()[1].name(), crate::schema::node::CANVAS);
}

#[test]
fn link_cards_parse_and_serialize() {
    use crate::schema::node;
    let md = "[Example](https://example.com \"kawruh:card\")\n\n[Big](https://example.com/x \"kawruh:preview\")\n";
    let p = markdown_to_pnode(md);
    let card = &p.children()[0];
    assert_eq!(card.name(), node::LINK_CARD);
    assert_eq!(card.attr_str("href"), "https://example.com");
    assert_eq!(card.attr_str("label"), "Example");
    assert_eq!(card.attr_str("view"), "card");
    assert_eq!(p.children()[1].attr_str("view"), "preview");
    contract::load().validate(&p).unwrap();
    assert_eq!(doc_to_markdown(&doc_from_md(md)), md);
}

#[test]
fn only_a_lone_marked_link_is_a_card() {
    use crate::schema::node;
    for md in [
        "[x](https://e.com \"kawruh:other\")",
        "[x](https://e.com \"Docs\")",
        "see [x](https://e.com \"kawruh:card\")",
        "<https://e.com>",
    ] {
        let p = markdown_to_pnode(md);
        assert_eq!(p.children()[0].name(), node::PARAGRAPH, "{md}");
    }
}

#[test]
fn image_srcs_can_be_rewritten() {
    let md = "Look ![a](kawruh-asset://w/1.png) and ![b](https://e.com/b.png)\n";
    let out = crate::markdown::map_image_srcs(md, |src| {
        src.strip_prefix("kawruh-asset://w/")
            .map(|name| format!("Page files/{name}"))
    });
    assert_eq!(
        out,
        "Look ![a](<Page files/1.png>) and ![b](https://e.com/b.png)\n"
    );
}

#[test]
fn link_hrefs_can_be_rewritten() {
    let md =
        "See [**it**](Other.md) and [web](https://e.com)\n\n[Card](Other.md \"kawruh:card\")\n";
    let out = crate::markdown::map_link_hrefs(md, |href| {
        (href == "Other.md").then(|| "kawruh://page/abc".to_string())
    });
    assert_eq!(
        out,
        "See [**it**](kawruh://page/abc) and [web](https://e.com)\n\n\
         [Card](kawruh://page/abc \"kawruh:card\")\n"
    );
}

#[test]
fn html_is_escaped_in_share_rendering() {
    let html = markdown_to_html("hi <script>alert(1)</script>");
    assert!(!html.contains("<script>"), "{html}");
}

/// Writes Rust-produced Yjs updates + markdown for the JS contract test
/// (`app/tests/contract.test.ts`), and checks updates the JS side wrote
/// (`*.js.bin`) decode into contract-valid trees.
#[test]
fn cross_language_fixtures() {
    let out = fixtures_dir().join("out");
    fs::create_dir_all(&out).unwrap();
    let c = contract::load();
    for (name, md) in fixtures() {
        let doc = doc_from_md(&md);
        let update = doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        fs::write(out.join(format!("{name}.rs.bin")), update).unwrap();
        fs::write(out.join(format!("{name}.rs.md")), doc_to_markdown(&doc)).unwrap();

        let js = out.join(format!("{name}.js.bin"));
        if let Ok(bytes) = fs::read(&js) {
            let jdoc = Doc::new();
            jdoc.transact_mut()
                .apply_update(Update::decode_v1(&bytes).unwrap())
                .unwrap();
            let p = read(&jdoc);
            c.validate(&p)
                .unwrap_or_else(|e| panic!("{name}.js.bin: {e}\n{p:#?}"));
            // The JS editor's view of the fixture must mean the same as ours.
            let ours = markdown_to_pnode(&md);
            let theirs_md = pnode_to_markdown(&p);
            assert_eq!(
                pnode_to_markdown(&ours),
                theirs_md,
                "{name}: Milkdown-produced doc serializes differently"
            );
        }
    }
}
