//! `PNode` → markdown (GFM), close to what Milkdown's remark-stringify emits.

use crate::schema::{CANVAS_LANG, PMark, PNode, mark, node};

pub fn pnode_to_markdown(doc: &PNode) -> String {
    let mut out = blocks(doc.children(), true);
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Serializes sibling blocks. Loose content is separated by a blank line,
/// tight list content by a single newline.
fn blocks(nodes: &[PNode], loose: bool) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(nodes.len());
    let mut alt = false;
    for (i, n) in nodes.iter().enumerate() {
        // Adjacent lists of the same kind would merge on re-parse unless the
        // marker changes, so alternate it (like remark-stringify does).
        let same_as_prev = i > 0 && nodes[i - 1].name() == n.name();
        alt = same_as_prev && !alt;
        let out = match n.name() {
            node::BULLET_LIST | node::ORDERED_LIST => list(n, alt),
            _ => block(n),
        };
        if !out.is_empty() {
            parts.push(out);
        }
    }
    parts.join(if loose { "\n\n" } else { "\n" })
}

fn indent(text: &str, first: &str, rest: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let prefix = if i == 0 { first } else { rest };
        if line.is_empty() {
            out.push_str(prefix.trim_end());
        } else {
            out.push_str(prefix);
            out.push_str(line);
        }
    }
    out
}

fn fence_for(content: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in content.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

fn block(n: &PNode) -> String {
    match n.name() {
        node::PARAGRAPH => inline_content(n.children(), false),
        node::HEADING => {
            let level = n.attr_num("level").unwrap_or(1.0).clamp(1.0, 6.0) as usize;
            let text = inline_content(n.children(), false).replace('\n', " ");
            format!("{} {}", "#".repeat(level), text)
                .trim_end()
                .to_string()
        }
        node::BLOCKQUOTE => indent(&blocks(n.children(), true), "> ", "> "),
        node::CODE_BLOCK => {
            let content = n.text_content();
            let fence = fence_for(&content);
            format!("{fence}{}\n{content}\n{fence}", n.attr_str("language"))
        }
        node::CANVAS => format!("```{CANVAS_LANG}\n{}\n```", n.attr_str("id")),
        node::HR => "---".to_string(),
        node::BULLET_LIST | node::ORDERED_LIST => list(n, false),
        node::TABLE => table(n),
        node::FOOTNOTE_DEFINITION => {
            let body = blocks(n.children(), true);
            indent(&body, &format!("[^{}]: ", n.attr_str("label")), "    ")
        }
        // Inline nodes that ended up at block level.
        _ => inline_content(std::slice::from_ref(n), false),
    }
}

fn list(n: &PNode, alt: bool) -> String {
    let ordered = n.name() == node::ORDERED_LIST;
    let start = n.attr_num("order").unwrap_or(1.0) as i64;
    let loose = n.attr_bool("spread").unwrap_or(false)
        || n.children()
            .iter()
            .any(|i| i.attr_bool("spread") == Some(true) && i.children().len() > 1);
    let items: Vec<String> = n
        .children()
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut marker = match (ordered, alt) {
                (true, false) => format!("{}. ", start + i as i64),
                (true, true) => format!("{}) ", start + i as i64),
                (false, false) => "- ".to_string(),
                (false, true) => "* ".to_string(),
            };
            let pad = " ".repeat(marker.len());
            match item.attr_bool("checked") {
                Some(true) => marker.push_str("[x] "),
                Some(false) => marker.push_str("[ ] "),
                None => {}
            }
            let body = blocks(item.children(), loose);
            indent(&body, &marker, &pad).trim_end().to_string()
        })
        .collect();
    items.join(if loose { "\n\n" } else { "\n" })
}

fn table(n: &PNode) -> String {
    let rows: Vec<Vec<String>> = n
        .children()
        .iter()
        .map(|row| {
            row.children()
                .iter()
                .map(|cell| {
                    let content: Vec<PNode> = cell
                        .children()
                        .iter()
                        .flat_map(|p| p.children().iter().cloned())
                        .collect();
                    inline_content(&content, true)
                })
                .collect()
        })
        .collect();
    let columns = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if columns == 0 {
        return String::new();
    }
    let aligns: Vec<&str> = (0..columns)
        .map(|i| {
            n.children()
                .first()
                .and_then(|r| r.children().get(i))
                .map(|c| c.attr_str("alignment"))
                .unwrap_or("left")
        })
        .collect();
    let line = |cells: &[String]| {
        let mut parts: Vec<String> = cells.to_vec();
        parts.resize(columns, String::new());
        format!("| {} |", parts.join(" | "))
    };
    let mut out = vec![line(&rows[0])];
    let sep: Vec<String> = aligns
        .iter()
        .map(|a| match *a {
            "center" => ":-:".to_string(),
            "right" => "--:".to_string(),
            _ => "---".to_string(),
        })
        .collect();
    out.push(format!("| {} |", sep.join(" | ")));
    for row in &rows[1..] {
        out.push(line(row));
    }
    out.join("\n")
}

fn mark_rank(name: &str) -> u8 {
    match name {
        mark::LINK => 0,
        mark::STRONG => 1,
        mark::EMPHASIS => 2,
        mark::STRIKE => 3,
        _ => 4,
    }
}

fn open_mark(m: &PMark) -> String {
    match m.name.as_str() {
        mark::STRONG => "**".into(),
        mark::EMPHASIS => "*".into(),
        mark::STRIKE => "~~".into(),
        mark::LINK => "[".into(),
        _ => String::new(),
    }
}

fn close_mark(m: &PMark) -> String {
    match m.name.as_str() {
        mark::STRONG => "**".into(),
        mark::EMPHASIS => "*".into(),
        mark::STRIKE => "~~".into(),
        mark::LINK => {
            let href = attr_string(m, "href");
            let title = attr_string(m, "title");
            if title.is_empty() {
                format!("]({})", link_dest(&href))
            } else {
                format!("]({} \"{}\")", link_dest(&href), title.replace('"', "\\\""))
            }
        }
        _ => String::new(),
    }
}

fn attr_string(m: &PMark, key: &str) -> String {
    match m.attrs.get(key) {
        Some(yrs::Any::String(s)) => s.to_string(),
        _ => String::new(),
    }
}

fn link_dest(url: &str) -> String {
    if url.is_empty() || url.contains(' ') || url.contains('(') || url.contains(')') {
        format!("<{}>", url.replace('<', "%3C").replace('>', "%3E"))
    } else {
        url.to_string()
    }
}

/// Escapes text for markdown. `line_start` carries whether the output is at the
/// start of a line across calls, since block syntax (`#`, `-`, `1.`) only
/// triggers there.
fn escape_text(text: &str, in_table: bool, line_start: &mut bool) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '\\' | '*' | '_' | '`' | '[' | ']' | '~' | '<' => {
                out.push('\\');
                out.push(c);
            }
            '|' if in_table => out.push_str("\\|"),
            '#' | '>' | '+' | '-' | '=' if *line_start => {
                out.push('\\');
                out.push(c);
            }
            '0'..='9' if *line_start => {
                // "1." at the start of a line would become an ordered list.
                let mut j = i;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                out.push(c);
                if j < chars.len() && (chars[j] == '.' || chars[j] == ')') && i + 1 == j {
                    out.push('\\');
                }
            }
            '\n' if in_table => out.push(' '),
            _ => out.push(c),
        }
        *line_start = c == '\n' || (*line_start && c == ' ');
    }
    out
}

fn code_span(text: &str) -> String {
    let fence = {
        let mut longest = 0;
        let mut run = 0;
        for c in text.chars() {
            if c == '`' {
                run += 1;
                longest = longest.max(run);
            } else {
                run = 0;
            }
        }
        "`".repeat(longest + 1)
    };
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// Marks that delimit (everything except inline code), outermost first.
fn delimiting_marks(marks: &[PMark]) -> Vec<PMark> {
    let mut m: Vec<PMark> = marks
        .iter()
        .filter(|m| m.name != mark::INLINE_CODE)
        .cloned()
        .collect();
    m.sort_by_key(|m| mark_rank(&m.name));
    m
}

fn common_prefix(a: &[PMark], b: &[PMark]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// Serializes inline content, opening/closing marks between runs.
fn inline_content(nodes: &[PNode], in_table: bool) -> String {
    let mut out = String::new();
    let mut open: Vec<PMark> = Vec::new();
    let mut at_line_start = !in_table;

    let close_to = |out: &mut String, open: &mut Vec<PMark>, keep: usize| {
        while open.len() > keep {
            let m = open.pop().unwrap();
            out.push_str(&close_mark(&m));
        }
    };

    for (idx, n) in nodes.iter().enumerate() {
        match n {
            PNode::Text { text, marks } => {
                let code = marks.iter().any(|m| m.name == mark::INLINE_CODE);
                let wanted = delimiting_marks(marks);
                let next = match nodes.get(idx + 1) {
                    Some(PNode::Text { marks, .. }) => delimiting_marks(marks),
                    _ => Vec::new(),
                };

                let keep = common_prefix(&open, &wanted);
                close_to(&mut out, &mut open, keep);

                // Delimiters can't hug whitespace: leading whitespace goes before
                // newly opened marks, trailing whitespace after marks that end here.
                let (lead, core, trail) = if code || wanted.len() == open.len() {
                    ("", text.as_str(), "")
                } else {
                    let trimmed_start = text.trim_start();
                    let lead = &text[..text.len() - trimmed_start.len()];
                    let core = trimmed_start.trim_end();
                    (lead, core, &trimmed_start[core.len()..])
                };
                out.push_str(&escape_text(lead, in_table, &mut at_line_start));
                if core.is_empty() {
                    out.push_str(&escape_text(trail, in_table, &mut at_line_start));
                    continue;
                }
                for m in &wanted[open.len()..] {
                    out.push_str(&open_mark(m));
                    open.push(m.clone());
                }
                if code {
                    out.push_str(&code_span(core));
                    at_line_start = false;
                } else {
                    out.push_str(&escape_text(core, in_table, &mut at_line_start));
                }
                if !trail.is_empty() {
                    let keep_next = common_prefix(&open, &next);
                    close_to(&mut out, &mut open, keep_next);
                    out.push_str(&escape_text(trail, in_table, &mut at_line_start));
                }
            }
            PNode::Element { name, .. } => {
                close_to(&mut out, &mut open, 0);
                match name.as_str() {
                    node::HARDBREAK => {
                        let soft = n.attr_bool("isInline") == Some(true);
                        out.push_str(match (in_table, soft) {
                            (true, true) => " ",
                            (true, false) => "<br>",
                            (false, true) => "\n",
                            (false, false) => "\\\n",
                        });
                        at_line_start = !in_table;
                        continue;
                    }
                    node::IMAGE => {
                        let alt = escape_text(n.attr_str("alt"), in_table, &mut false);
                        let src = link_dest(n.attr_str("src"));
                        let title = n.attr_str("title");
                        if title.is_empty() {
                            out.push_str(&format!("![{alt}]({src})"));
                        } else {
                            out.push_str(&format!(
                                "![{alt}]({src} \"{}\")",
                                title.replace('"', "\\\"")
                            ));
                        }
                    }
                    node::HTML => out.push_str(n.attr_str("value")),
                    node::FOOTNOTE_REFERENCE => {
                        out.push_str(&format!("[^{}]", n.attr_str("label")))
                    }
                    _ => out.push_str(&escape_text(&n.text_content(), in_table, &mut false)),
                }
                at_line_start = false;
            }
        }
    }
    close_to(&mut out, &mut open, 0);
    out
}
