//! Public read-only share pages (`/s/:token`, `/s/:token/:page_id`) and the
//! images they embed (`/s/:token/assets/:name`).

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use base64::Engine;
use serde_json::Value;
use yrs::{Map, Out, Transact};

use kawruh_core::workspace::{self, PageMeta};

use crate::assets;
use crate::db;
use crate::state::AppState;

pub const CANVAS_SVG: &str = "canvas_svg";
/// Pages link to each other in markdown as `[Title](kawruh://page/<page_id>)`.
pub const PAGE_LINK: &str = "kawruh://page/";
/// Root map of a page doc: link href → JSON `{title, description, image, icon, site}`.
pub const LINK_META: &str = "link_meta";
const MERMAID_ESM: &str = "https://cdn.jsdelivr.net/npm/mermaid@12.2.0/dist/mermaid.esm.min.mjs";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Undoes HTML escaping (named `amp lt gt quot apos` and numeric references),
/// as comrak applies it to text and attribute values.
fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let decoded = tail.find(';').filter(|&end| end <= 10).and_then(|end| {
            let c = match &tail[1..end] {
                "amp" => '&',
                "lt" => '<',
                "gt" => '>',
                "quot" => '"',
                "apos" => '\'',
                ent => {
                    let code = match ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                        Some(h) => u32::from_str_radix(h, 16).ok(),
                        None => ent.strip_prefix('#').and_then(|d| d.parse().ok()),
                    };
                    char::from_u32(code?)?
                }
            };
            Some((c, end + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn not_found() -> Response {
    let body = page_shell(
        "Not found",
        "<p class=\"muted\">This share link does not exist or the page is no longer shared.</p>",
    );
    (StatusCode::NOT_FOUND, Html(body)).into_response()
}

/// The share behind `token` and the workspace's live pages, while the shared
/// page still exists.
fn resolve(state: &AppState, token: &str) -> Option<(db::Share, Vec<PageMeta>)> {
    let share = {
        let conn = state.db.lock();
        db::share_by_token(&conn, token).ok().flatten()
    }?;
    let ws = state
        .docs
        .get_or_load(&share.workspace_id, &share.workspace_id, false)
        .ok()?;
    let pages = workspace::live_pages(&ws.doc);
    if !pages.iter().any(|p| p.id == share.page_id) {
        return None;
    }
    Some((share, pages))
}

/// An image of the shared page's workspace.
pub async fn asset(
    State(state): State<AppState>,
    Path((token, name)): Path<(String, String)>,
) -> Response {
    let Some((share, _)) = resolve(&state, &token) else {
        return not_found();
    };
    let Some((_, content_type)) = assets::parse_name(&name) else {
        return not_found();
    };
    match assets::read(&state.cfg.data_dir, &share.workspace_id, &name).await {
        Ok(Some(bytes)) => assets::serve(bytes, content_type, assets::CACHE_PUBLIC),
        _ => not_found(),
    }
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

/// Ids of `root` and all its descendants: the pages a share link reaches.
fn subtree(pages: &[PageMeta], root: &str) -> HashSet<String> {
    let mut out = HashSet::from([root.to_string()]);
    let mut stack = vec![root.to_string()];
    while let Some(id) = stack.pop() {
        for p in pages.iter().filter(|p| p.parent == id) {
            if out.insert(p.id.clone()) {
                stack.push(p.id.clone());
            }
        }
    }
    out
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
    let Some((share, pages)) = resolve(&state, &token) else {
        return not_found();
    };
    let page_id = sub_page.unwrap_or_else(|| share.page_id.clone());
    let Some(page) = pages.iter().find(|p| p.id == page_id) else {
        return not_found();
    };
    // Only the shared page and its descendants are reachable.
    let reachable = subtree(&pages, &share.page_id);
    if !reachable.contains(&page_id) {
        return not_found();
    }
    let chain = ancestors(&pages, &page_id);
    let link = |id: &str| {
        if id == share.page_id {
            format!("/s/{token}")
        } else {
            format!("/s/{token}/{id}")
        }
    };

    let Ok(live) = state.docs.get_or_load(&page_id, &share.workspace_id, false) else {
        return not_found();
    };
    let markdown = kawruh_core::markdown::doc_to_markdown(&live.doc);
    // A database reads as a table, not as its JSON block.
    let markdown = if page.kind == kawruh_core::database::KIND_DATABASE {
        match kawruh_core::database::to_markdown(&live.doc) {
            Some(table) if markdown.trim().is_empty() => table,
            Some(table) => format!("{markdown}\n\n{table}"),
            None => markdown,
        }
    } else {
        markdown
    };
    let body_html = embed_canvases(
        &kawruh_core::markdown::markdown_to_html(&markdown),
        &string_map(&live.doc, CANVAS_SVG),
    );
    let body_html = rewrite_assets(&body_html, &share.workspace_id, &token);
    let body_html = render_cards(&body_html, &string_map(&live.doc, LINK_META));
    let body_html = rewrite_page_links(&body_html, |id| reachable.contains(id).then(|| link(id)));
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
            "default-src 'none'; script-src 'unsafe-inline' https://cdn.jsdelivr.net; style-src 'unsafe-inline'; img-src 'self' data: https: http:; font-src data:; base-uri 'none'; form-action 'none'",
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

/// String entries of a root map of the document.
fn string_map(doc: &yrs::Doc, name: &str) -> HashMap<String, String> {
    let map = doc.get_or_insert_map(name);
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
fn embed_canvases(html: &str, svgs: &HashMap<String, String>) -> String {
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

/// Points image sources `kawruh-asset://<this workspace>/<name>` at the share's
/// public asset route. References to other workspaces are left untouched.
fn rewrite_assets(html: &str, workspace_id: &str, token: &str) -> String {
    let prefix = format!("src=\"{}", assets::SCHEME);
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(&prefix) {
        let after = &rest[start + prefix.len()..];
        let Some(end) = after.find('"') else {
            break;
        };
        out.push_str(&rest[..start]);
        match after[..end].split_once('/') {
            Some((ws, name)) if ws == workspace_id && assets::parse_name(name).is_some() => {
                out.push_str(&format!("src=\"/s/{}/assets/{name}\"", esc(token)));
            }
            _ => out.push_str(&rest[start..start + prefix.len() + end + 1]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Points page links (`kawruh://page/<id>`) at the page within this share
/// when `href_of` gives its URL; otherwise only the link text stays. No other
/// `kawruh:` link survives either, so share HTML never holds such an href.
fn rewrite_page_links(html: &str, href_of: impl Fn(&str) -> Option<String>) -> String {
    const OPEN: &str = "<a href=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let after = &rest[start + OPEN.len()..];
        let Some(quote) = after.find('"') else {
            rest = &rest[start..];
            break;
        };
        let href = unesc(&after[..quote]);
        if !href
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("kawruh:")
        {
            out.push_str(&rest[start..start + OPEN.len() + quote + 1]);
            rest = &after[quote + 1..];
            continue;
        }
        let target = href
            .strip_prefix(PAGE_LINK)
            .and_then(|r| r.split(['/', '?', '#']).next())
            .filter(|id| crate::api::valid_id(id))
            .and_then(&href_of);
        match target {
            Some(url) => {
                out.push_str(&format!("{OPEN}{}\"", esc(&url)));
                rest = &after[quote + 1..];
            }
            None => {
                // Drop the opening tag and its `</a>` (links cannot nest).
                let inner = &after[quote..];
                let inner = inner.find('>').map_or("", |i| &inner[i + 1..]);
                match inner.find("</a>") {
                    Some(end) => {
                        out.push_str(&inner[..end]);
                        rest = &inner[end + "</a>".len()..];
                    }
                    None => rest = inner,
                }
            }
        }
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CardKind {
    /// Compact: text with the image on the right.
    Card,
    /// Large image on top.
    Preview,
}

/// A paragraph that holds nothing but a link titled `kawruh:card` or
/// `kawruh:preview`, as comrak renders it.
struct CardLink<'a> {
    kind: CardKind,
    /// The href with comrak's HTML escaping undone.
    href: String,
    /// The link's inner HTML (comrak output, raw HTML already escaped).
    inner: &'a str,
}

/// Parses a card paragraph at the start of `s`; returns it and its length.
fn parse_card(s: &str) -> Option<(CardLink<'_>, usize)> {
    let rest = s.strip_prefix("<p><a href=\"")?;
    let quote = rest.find('"')?;
    let href = &rest[..quote];
    let rest = &rest[quote..];
    let (kind, rest) = match rest.strip_prefix("\" title=\"kawruh:card\">") {
        Some(r) => (CardKind::Card, r),
        None => (
            CardKind::Preview,
            rest.strip_prefix("\" title=\"kawruh:preview\">")?,
        ),
    };
    // Links cannot nest, so the first `</a>` closes this one.
    let end = rest.find("</a>")?;
    let tail = rest[end..].strip_prefix("</a></p>")?;
    let link = CardLink {
        kind,
        href: unesc(href),
        inner: &rest[..end],
    };
    Some((link, s.len() - tail.len()))
}

/// Renders card paragraphs as link cards using the page's `link_meta`.
fn render_cards(html: &str, meta: &HashMap<String, String>) -> String {
    const OPEN: &str = "<p><a href=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        let at = &rest[start..];
        match parse_card(at) {
            Some((link, len)) => {
                out.push_str(&card_html(&link, meta));
                rest = &at[len..];
            }
            None => {
                out.push_str(OPEN);
                rest = &at[OPEN.len()..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn card_html(link: &CardLink, meta: &HashMap<String, String>) -> String {
    let Some(href) = web_url(&link.href) else {
        // Only http(s) links become cards; anything else loses its link.
        return format!("<p>{}</p>", link.inner);
    };
    let meta: Value = lookup_meta(meta, &link.href)
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or(Value::Null);
    let field = |key: &str| {
        meta.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let domain = host_of(&href);
    let text = plain_text(link.inner);
    let title = match field("title") {
        Some(t) => t.to_string(),
        None if !text.is_empty() => text,
        None => domain.clone(),
    };
    let site = field("site").unwrap_or(&domain);

    let image = field("image")
        .and_then(web_url)
        .map(|src| {
            format!(
                "<span class=\"lc-image\"><img src=\"{}\" alt=\"\" loading=\"lazy\"></span>",
                esc(&src)
            )
        })
        .unwrap_or_default();
    let icon = field("icon")
        .and_then(web_url)
        .map(|src| {
            format!(
                "<img class=\"lc-icon\" src=\"{}\" alt=\"\" width=\"16\" height=\"16\" loading=\"lazy\">",
                esc(&src)
            )
        })
        .unwrap_or_default();
    let mut body = format!(
        "<span class=\"lc-title\">{}</span>",
        esc(&clip(&title, 300))
    );
    if let Some(desc) = field("description") {
        body.push_str(&format!(
            "<span class=\"lc-desc\">{}</span>",
            esc(&clip(desc, 500))
        ));
    }
    body.push_str(&format!(
        "<span class=\"lc-site\">{icon}<span>{}</span></span>",
        esc(&clip(site, 100))
    ));
    let (class, before, after) = match link.kind {
        CardKind::Card => ("link-card", "", image.as_str()),
        CardKind::Preview => ("link-card link-card-preview", image.as_str(), ""),
    };
    format!(
        "<a class=\"{class}\" href=\"{}\" rel=\"noopener noreferrer nofollow\">{before}<span class=\"lc-body\">{body}</span>{after}</a>",
        esc(&href)
    )
}

/// The metadata JSON stored for `href`. Keys are the raw hrefs the editor
/// saw; comrak percent-encodes some characters, so compare decoded forms too.
fn lookup_meta<'a>(meta: &'a HashMap<String, String>, href: &str) -> Option<&'a String> {
    meta.get(href).or_else(|| {
        let want = pct_decode(href);
        meta.iter()
            .find(|(k, _)| pct_decode(k) == want)
            .map(|(_, v)| v)
    })
}

/// `s` (trimmed) when it is an absolute http(s) URL with a host.
fn web_url(s: &str) -> Option<String> {
    let s = s.trim();
    let head = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    let rest = if head.starts_with("https://") {
        &s[8..]
    } else if head.starts_with("http://") {
        &s[7..]
    } else {
        return None;
    };
    if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.rsplit('@').next().unwrap_or("").is_empty() {
        return None;
    }
    Some(s.to_string())
}

/// Display host of an http(s) URL, without `www.`.
fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let host = if host.starts_with('[') {
        host.split_inclusive(']').next().unwrap_or(host)
    } else {
        host.split(':').next().unwrap_or(host)
    };
    let host = pct_decode(host).to_lowercase();
    match host.strip_prefix("www.") {
        Some(h) => h.to_string(),
        None => host,
    }
}

fn pct_decode(s: &str) -> String {
    let hex = |b: Option<&u8>| b.and_then(|&b| (b as char).to_digit(16)).map(|d| d as u8);
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(hi), Some(lo)) = (hex(bytes.get(i + 1)), hex(bytes.get(i + 2)))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Text content of an HTML fragment produced by comrak.
fn plain_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    unesc(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// At most `max` characters of `s`, with an ellipsis when cut.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", s[..i].trim_end()),
        None => s.to_string(),
    }
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
a.link-card {{ display: flex; margin: 0 0 1.1rem; min-height: 5.5rem; border: 1px solid var(--line); border-radius: 10px; background: var(--card); color: var(--fg); text-decoration: none; overflow: hidden; font-family: ui-sans-serif, -apple-system, "Segoe UI", system-ui, sans-serif; }}
a.link-card:hover {{ border-color: var(--accent); }}
.lc-body {{ flex: 1; min-width: 0; display: flex; flex-direction: column; gap: .2rem; padding: .75rem 1rem; }}
.lc-title {{ font-weight: 600; font-size: .98rem; line-height: 1.35; display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; overflow-wrap: anywhere; }}
.lc-desc {{ font-size: .86rem; line-height: 1.45; color: var(--muted); display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; overflow-wrap: anywhere; }}
.lc-site {{ display: flex; align-items: center; gap: .4rem; margin-top: auto; padding-top: .2rem; font-size: .8rem; color: var(--muted); min-width: 0; }}
.lc-site span {{ white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }}
.lc-icon {{ width: 16px; height: 16px; flex: none; border-radius: 3px; }}
.lc-image {{ flex: none; width: 30%; max-width: 12rem; background: var(--code-bg); border-left: 1px solid var(--line); }}
.lc-image img {{ display: block; width: 100%; height: 100%; max-width: none; object-fit: cover; }}
a.link-card-preview {{ flex-direction: column; }}
a.link-card-preview .lc-image {{ width: 100%; max-width: none; aspect-ratio: 1.91 / 1; border-left: 0; border-bottom: 1px solid var(--line); }}
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
        let mut svgs = HashMap::new();
        svgs.insert("c1".to_string(), "<svg/>".to_string());
        let html = "<p>a</p>\n<pre><code class=\"language-kawruh-canvas\">c1\n</code></pre>\n<pre><code class=\"language-kawruh-canvas\">zz\n</code></pre>";
        let out = embed_canvases(html, &svgs);
        assert!(out.contains("data:image/svg+xml;base64,"));
        assert!(out.contains("Canvas drawing"));
        assert!(!out.contains("language-kawruh-canvas"));
    }

    const NAME: &str = "3f79bb7b435b05321651daefd374cdc681dc06faa65e374e38337b88ca046dea.png";

    #[test]
    fn asset_sources_of_the_shared_workspace_are_rewritten() {
        let md = format!(
            "![a](kawruh-asset://ws1/{NAME}) ![b](kawruh-asset://ws2/{NAME}) ![c](kawruh-asset://ws1/nope.png)\n\n`src=\"kawruh-asset://ws1/{NAME}\"`\n"
        );
        let html = kawruh_core::markdown::markdown_to_html(&md);
        let out = rewrite_assets(&html, "ws1", "tok");
        assert!(
            out.contains(&format!("<img src=\"/s/tok/assets/{NAME}\" alt=\"a\"")),
            "{out}"
        );
        assert!(out.contains(&format!("src=\"kawruh-asset://ws2/{NAME}\"")));
        assert!(out.contains("src=\"kawruh-asset://ws1/nope.png\""));
        // Text that merely looks like an attribute is escaped and left alone.
        assert!(out.contains("src=&quot;kawruh-asset://ws1/"), "{out}");
    }

    fn cards(md: &str, meta: &[(&str, &str)]) -> String {
        let meta = meta
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        render_cards(&kawruh_core::markdown::markdown_to_html(md), &meta)
    }

    #[test]
    fn card_without_metadata_shows_title_and_domain() {
        let out = cards(
            "[Example Domain](https://www.Example.com/a?b=1&c=2 \"kawruh:card\")\n",
            &[],
        );
        assert!(
            out.starts_with(
                "<a class=\"link-card\" href=\"https://www.Example.com/a?b=1&amp;c=2\" rel=\"noopener noreferrer nofollow\">"
            ),
            "{out}"
        );
        assert!(out.contains("<span class=\"lc-title\">Example Domain</span>"));
        assert!(out.contains("<span>example.com</span>"));
        assert!(!out.contains("<img") && !out.contains("<p>"), "{out}");
    }

    #[test]
    fn card_metadata_is_escaped_and_urls_filtered() {
        let meta = r#"{"title": "<script>alert(1)</script>", "description": "a \"quoted\" & <b>bold</b>", "image": "https://img.test/og.png?a=1&b=\"x\"", "icon": "javascript:alert(2)", "site": "Site <i>"}"#;
        let out = cards(
            "[Text](https://example.com/p?q=1&r=2 \"kawruh:card\")\n",
            &[("https://example.com/p?q=1&r=2", meta)],
        );
        assert!(
            out.contains("<span class=\"lc-title\">&lt;script&gt;alert(1)&lt;/script&gt;</span>"),
            "{out}"
        );
        assert!(out.contains("a &quot;quoted&quot; &amp; &lt;b&gt;bold&lt;/b&gt;"));
        assert!(out.contains("src=\"https://img.test/og.png?a=1&amp;b=&quot;x&quot;\""));
        assert!(out.contains("<span>Site &lt;i&gt;</span>"));
        assert!(
            !out.contains("javascript:") && !out.contains("<script"),
            "{out}"
        );
        assert!(!out.contains("lc-icon"));
        // Compact cards put the image after the text.
        assert!(out.find("lc-body").unwrap() < out.find("lc-image").unwrap());
    }

    #[test]
    fn preview_cards_put_the_image_on_top() {
        let meta = r#"{"title": "Big", "image": "http://img.test/big.png", "icon": "https://img.test/fav.ico"}"#;
        let out = cards(
            "[x](https://blog.test/post \"kawruh:preview\")\n",
            &[("https://blog.test/post", meta)],
        );
        assert!(
            out.starts_with("<a class=\"link-card link-card-preview\""),
            "{out}"
        );
        assert!(out.find("lc-image").unwrap() < out.find("lc-body").unwrap());
        assert!(out.contains("<img class=\"lc-icon\" src=\"https://img.test/fav.ico\""));
        assert!(out.contains("<span>blog.test</span>"));
    }

    #[test]
    fn non_web_card_links_are_not_rendered_as_links() {
        for md in [
            "[Bad](javascript:alert(1) \"kawruh:card\")\n",
            "[Bad](JAVASCRIPT:alert(1) \"kawruh:preview\")\n",
            "[Bad](mailto:a@b.c \"kawruh:card\")\n",
            "[Bad](/relative \"kawruh:card\")\n",
        ] {
            let out = cards(md, &[]);
            assert_eq!(out.trim(), "<p>Bad</p>", "{md}");
        }
    }

    #[test]
    fn only_lone_card_links_become_cards() {
        let md = "See [a](https://a.test \"kawruh:card\") inline.\n\n[b](https://b.test \"other\")\n\n[c](https://c.test \"kawruh:card\") [d](https://d.test \"kawruh:card\")\n";
        let html = kawruh_core::markdown::markdown_to_html(md);
        assert_eq!(render_cards(&html, &HashMap::new()), html);
    }

    #[test]
    fn metadata_matches_percent_encoded_hrefs() {
        let out = cards(
            "[x](https://例え.test/パス \"kawruh:card\")\n",
            &[("https://例え.test/パス", r#"{"title": "Found"}"#)],
        );
        assert!(out.contains(">Found</span>"), "{out}");
        assert!(out.contains("<span>例え.test</span>"), "{out}");
    }

    #[test]
    fn page_links_point_into_the_share_or_lose_their_link() {
        let md = "[In](kawruh://page/p1) [Titled](kawruh://page/p1 \"tip\") [Out](kawruh://page/p9) \
                  [Bad](kawruh://page/../x) [Odd](KAWRUH://page/p1) [Other](kawruh://elsewhere) \
                  [Web](https://example.com) [Rel](/s/x)[^n]\n\n[^n]: note\n";
        let html = kawruh_core::markdown::markdown_to_html(md);
        let out = rewrite_page_links(&html, |id| (id == "p1").then(|| "/s/t&k/p1".to_string()));
        assert!(out.contains("<a href=\"/s/t&amp;k/p1\">In</a>"), "{out}");
        assert!(
            out.contains("<a href=\"/s/t&amp;k/p1\" title=\"tip\">Titled</a>"),
            "{out}"
        );
        for text in [" Out ", " Bad ", " Odd ", " Other "] {
            assert!(out.contains(text), "{text} in {out}");
        }
        assert!(out.contains("<a href=\"https://example.com\">Web</a>"));
        assert!(out.contains("<a href=\"/s/x\">Rel</a>"));
        assert!(out.contains("href=\"#fn-n\""), "{out}");
        assert!(!out.to_lowercase().contains("kawruh:"), "{out}");
        // Unterminated markup is passed through without panicking.
        assert_eq!(
            rewrite_page_links("<a href=\"kawruh", |_| None),
            "<a href=\"kawruh"
        );
        assert_eq!(
            rewrite_page_links("<a href=\"kawruh://page/p1\">x", |_| None),
            "x"
        );
    }

    #[test]
    fn subtree_holds_the_root_and_its_descendants() {
        let page = |id: &str, parent: &str| PageMeta {
            id: id.into(),
            title: String::new(),
            parent: parent.into(),
            order: 0.0,
            icon: String::new(),
            created: 0,
            deleted: 0,
            kind: String::new(),
        };
        let pages = [page("a", ""), page("b", "a"), page("c", "b"), page("d", "")];
        let mut ids: Vec<String> = subtree(&pages, "a").into_iter().collect();
        ids.sort();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn unesc_handles_named_and_numeric_references() {
        assert_eq!(
            unesc("a&amp;b&lt;&gt;&quot;&#39;&#x27;&#65;&bogus;&"),
            "a&b<>\"''A&bogus;&"
        );
    }
}
