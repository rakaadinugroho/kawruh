//! Public read-only share pages (`/s/:token`, `/s/:token/:page_id`).

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use base64::Engine;
use yrs::{Map, Out, Transact};

use kawruh_core::workspace::{self, PageMeta};

use crate::db;
use crate::state::AppState;

pub const CANVAS_SVG: &str = "canvas_svg";
const MERMAID_ESM: &str = "https://cdn.jsdelivr.net/npm/mermaid@12.2.0/dist/mermaid.esm.min.mjs";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn unesc(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn not_found() -> Response {
    let body = page_shell(
        "Not found",
        "<p class=\"muted\">This share link does not exist or the page is no longer shared.</p>",
    );
    (StatusCode::NOT_FOUND, Html(body)).into_response()
}

pub async fn root(State(state): State<AppState>, Path(token): Path<String>) -> Response {
    render(state, token, None).await
}

pub async fn sub(
    State(state): State<AppState>,
    Path((token, page_id)): Path<(String, String)>,
) -> Response {
    render(state, token, Some(page_id)).await
}

/// Ancestors of `id` up to the root, nearest first.
fn ancestors<'a>(pages: &'a [PageMeta], id: &str) -> Vec<&'a PageMeta> {
    let mut out = Vec::new();
    let mut cur = pages.iter().find(|p| p.id == id).map(|p| p.parent.clone());
    while let Some(parent) = cur.filter(|p| !p.is_empty()) {
        let Some(p) = pages.iter().find(|p| p.id == parent) else {
            break;
        };
        if out.iter().any(|x: &&PageMeta| x.id == p.id) {
            break;
        }
        out.push(p);
        cur = Some(p.parent.clone());
    }
    out
}

async fn render(state: AppState, token: String, sub_page: Option<String>) -> Response {
    let share = {
        let conn = state.db.lock();
        db::share_by_token(&conn, &token).ok().flatten()
    };
    let Some(share) = share else {
        return not_found();
    };
    let Ok(ws) = state
        .docs
        .get_or_load(&share.workspace_id, &share.workspace_id, false)
    else {
        return not_found();
    };
    let pages = workspace::live_pages(&ws.doc);
    if !pages.iter().any(|p| p.id == share.page_id) {
        return not_found();
    }
    let page_id = sub_page.unwrap_or_else(|| share.page_id.clone());
    let Some(page) = pages.iter().find(|p| p.id == page_id) else {
        return not_found();
    };
    // Only the shared page and its descendants are reachable.
    let chain = ancestors(&pages, &page_id);
    if page_id != share.page_id && !chain.iter().any(|p| p.id == share.page_id) {
        return not_found();
    }

    let Ok(live) = state.docs.get_or_load(&page_id, &share.workspace_id, false) else {
        return not_found();
    };
    let markdown = kawruh_core::markdown::doc_to_markdown(&live.doc);
    let svgs = canvas_svgs(&live.doc);
    let body_html = embed_canvases(&kawruh_core::markdown::markdown_to_html(&markdown), &svgs);

    let link = |id: &str| {
        if id == share.page_id {
            format!("/s/{token}")
        } else {
            format!("/s/{token}/{id}")
        }
    };
    let title_of = |p: &PageMeta| {
        if p.title.trim().is_empty() {
            "Untitled".to_string()
        } else {
            p.title.clone()
        }
    };

    let mut crumbs = String::new();
    if page_id != share.page_id {
        let mut trail: Vec<&PageMeta> = chain
            .iter()
            .take_while(|p| p.id != share.page_id)
            .copied()
            .collect();
        if let Some(root) = pages.iter().find(|p| p.id == share.page_id) {
            trail.push(root);
        }
        trail.reverse();
        crumbs.push_str("<nav class=\"crumbs\">");
        for p in trail {
            crumbs.push_str(&format!(
                "<a href=\"{}\">{}</a><span>/</span>",
                esc(&link(&p.id)),
                esc(&title_of(p))
            ));
        }
        crumbs.push_str("</nav>");
    }

    let children: Vec<&PageMeta> = pages.iter().filter(|p| p.parent == page_id).collect();
    let mut subpages = String::new();
    if !children.is_empty() {
        subpages.push_str("<section class=\"subpages\"><h2>Pages</h2><ul>");
        for c in children {
            let icon = if c.icon.is_empty() {
                "📄".to_string()
            } else {
                esc(&c.icon)
            };
            subpages.push_str(&format!(
                "<li><a href=\"{}\"><span class=\"icon\">{icon}</span>{}</a></li>",
                esc(&link(&c.id)),
                esc(&title_of(c))
            ));
        }
        subpages.push_str("</ul></section>");
    }

    let icon = if page.icon.is_empty() {
        String::new()
    } else {
        format!("<div class=\"page-icon\">{}</div>", esc(&page.icon))
    };
    let content = format!(
        "{crumbs}<header>{icon}<h1 class=\"title\">{}</h1></header><article>{body_html}</article>{subpages}",
        esc(&title_of(page))
    );
    let mut res = Html(page_shell(&title_of(page), &content)).into_response();
    res.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'unsafe-inline' https://cdn.jsdelivr.net; style-src 'unsafe-inline'; img-src data: https: http:; font-src data:; base-uri 'none'; form-action 'none'",
        ),
    );
    res.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res.headers_mut()
        .insert("Referrer-Policy", HeaderValue::from_static("no-referrer"));
    res
}

fn canvas_svgs(doc: &yrs::Doc) -> std::collections::HashMap<String, String> {
    let map = doc.get_or_insert_map(CANVAS_SVG);
    let txn = doc.transact();
    map.iter(&txn)
        .filter_map(|(k, v)| match v {
            Out::Any(yrs::Any::String(s)) => Some((k.to_string(), s.to_string())),
            _ => None,
        })
        .collect()
}

/// Replaces `kawruh-canvas` code blocks with the stored SVG preview (as an
/// `<img>` data URL, so any script inside the SVG never runs).
fn embed_canvases(html: &str, svgs: &std::collections::HashMap<String, String>) -> String {
    const OPEN: &str = "<pre><code class=\"language-kawruh-canvas\">";
    const CLOSE: &str = "</code></pre>";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let Some(end) = after.find(CLOSE) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let id = unesc(after[..end].trim());
        match svgs.get(&id) {
            Some(svg) => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
                out.push_str(&format!(
                    "<figure class=\"canvas\"><img alt=\"Canvas\" src=\"data:image/svg+xml;base64,{b64}\"></figure>"
                ));
            }
            None => out.push_str(
                "<figure class=\"canvas empty\">Canvas drawing (open in Kawruh to view)</figure>",
            ),
        }
        rest = &after[end + CLOSE.len()..];
    }
    out.push_str(rest);
    out
}

fn page_shell(title: &str, content: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>{title} · Kawruh</title>
<style>
:root {{
  --bg: #fbfaf7; --fg: #24221f; --muted: #7a756c; --line: #e7e3da;
  --accent: #2f5d8a; --code-bg: #f1eee7; --card: #ffffff;
  color-scheme: light;
}}
@media (prefers-color-scheme: dark) {{
  :root {{
    --bg: #191816; --fg: #e9e5dc; --muted: #9b958a; --line: #34312c;
    --accent: #8db6dd; --code-bg: #24221f; --card: #201f1c;
    color-scheme: dark;
  }}
}}
* {{ box-sizing: border-box; }}
html {{ -webkit-text-size-adjust: 100%; }}
body {{
  margin: 0; background: var(--bg); color: var(--fg);
  font: 17px/1.7 ui-serif, "Iowan Old Style", "Charter", Georgia, serif;
}}
main {{ max-width: 46rem; margin: 0 auto; padding: 3.5rem 1rem 5rem; }}
header {{ margin-bottom: 1.5rem; }}
.page-icon {{ font-size: 2.6rem; line-height: 1; margin-bottom: .5rem; }}
h1.title {{ font: 700 2.4rem/1.15 ui-sans-serif, -apple-system, "Segoe UI", system-ui, sans-serif; letter-spacing: -.02em; margin: 0; }}
h1, h2, h3, h4, h5, h6 {{ font-family: ui-sans-serif, -apple-system, "Segoe UI", system-ui, sans-serif; line-height: 1.25; letter-spacing: -.01em; }}
article h1 {{ font-size: 1.9rem; margin: 2.2rem 0 .8rem; }}
article h2 {{ font-size: 1.5rem; margin: 2rem 0 .7rem; }}
article h3 {{ font-size: 1.2rem; margin: 1.6rem 0 .5rem; }}
a {{ color: var(--accent); text-underline-offset: 2px; }}
p, ul, ol, blockquote, pre, table, figure {{ margin: 0 0 1.1rem; }}
blockquote {{ border-left: 3px solid var(--line); padding: .1rem 0 .1rem 1rem; color: var(--muted); }}
code {{ font: .88em/1.5 ui-monospace, "SF Mono", Menlo, Consolas, monospace; background: var(--code-bg); padding: .12em .35em; border-radius: 4px; }}
pre {{ background: var(--code-bg); padding: .9rem 1rem; border-radius: 8px; overflow-x: auto; }}
pre code {{ background: none; padding: 0; }}
hr {{ border: 0; border-top: 1px solid var(--line); margin: 2rem 0; }}
img {{ max-width: 100%; height: auto; }}
table {{ border-collapse: collapse; display: block; overflow-x: auto; max-width: 100%; }}
th, td {{ border: 1px solid var(--line); padding: .4rem .7rem; text-align: left; }}
th {{ font-family: ui-sans-serif, -apple-system, system-ui, sans-serif; font-size: .92em; }}
li > input[type=checkbox] {{ margin-right: .4rem; }}
ul.contains-task-list, ul:has(> li > input[type=checkbox]) {{ list-style: none; padding-left: 1.2rem; }}
figure.canvas {{ background: var(--card); border: 1px solid var(--line); border-radius: 10px; padding: .75rem; text-align: center; }}
figure.canvas.empty {{ color: var(--muted); font-size: .9rem; padding: 2rem 1rem; }}
.mermaid {{ text-align: center; margin: 0 0 1.1rem; overflow-x: auto; }}
.crumbs {{ font: .9rem ui-sans-serif, -apple-system, system-ui, sans-serif; color: var(--muted); margin-bottom: 1.5rem; display: flex; flex-wrap: wrap; gap: .35rem; }}
.crumbs span {{ color: var(--line); }}
.subpages {{ margin-top: 3rem; border-top: 1px solid var(--line); padding-top: 1rem; }}
.subpages h2 {{ font-size: .8rem; text-transform: uppercase; letter-spacing: .08em; color: var(--muted); margin: 0 0 .5rem; }}
.subpages ul {{ list-style: none; padding: 0; margin: 0; }}
.subpages li a {{ display: flex; gap: .5rem; padding: .35rem 0; text-decoration: none; color: var(--fg); font-family: ui-sans-serif, -apple-system, system-ui, sans-serif; }}
.subpages li a:hover {{ color: var(--accent); }}
.muted {{ color: var(--muted); }}
footer {{ margin-top: 4rem; font: .8rem ui-sans-serif, system-ui, sans-serif; color: var(--muted); }}
@media (max-width: 600px) {{ body {{ font-size: 16px; }} main {{ padding-top: 2rem; }} h1.title {{ font-size: 1.9rem; }} }}
</style>
</head>
<body>
<main>
{content}
<footer>Shared from Kawruh · read-only</footer>
</main>
<script type="module">
const blocks = document.querySelectorAll('pre > code.language-mermaid');
if (blocks.length) {{
  const dark = matchMedia('(prefers-color-scheme: dark)').matches;
  const {{ default: mermaid }} = await import('{MERMAID_ESM}');
  mermaid.initialize({{ startOnLoad: false, securityLevel: 'strict', theme: dark ? 'dark' : 'default' }});
  for (const code of blocks) {{
    const div = document.createElement('div');
    div.className = 'mermaid';
    div.textContent = code.textContent;
    code.parentElement.replaceWith(div);
  }}
  await mermaid.run({{ querySelector: '.mermaid' }});
}}
</script>
</body>
</html>"#,
        title = esc(title),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_blocks_become_images() {
        let mut svgs = std::collections::HashMap::new();
        svgs.insert("c1".to_string(), "<svg/>".to_string());
        let html = "<p>a</p>\n<pre><code class=\"language-kawruh-canvas\">c1\n</code></pre>\n<pre><code class=\"language-kawruh-canvas\">zz\n</code></pre>";
        let out = embed_canvases(html, &svgs);
        assert!(out.contains("data:image/svg+xml;base64,"));
        assert!(out.contains("Canvas drawing"));
        assert!(!out.contains("language-kawruh-canvas"));
    }
}
