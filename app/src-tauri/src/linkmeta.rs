//! Link previews: fetches a pasted URL and reads what a card shows (title,
//! description, image, icon, site name) from its OpenGraph/Twitter/HTML tags.

use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use regex::Regex;
use reqwest::Url;
use serde::Serialize;

use crate::error::{Error, Result};

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct LinkMeta {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub image: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub icon: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub site: String,
}

const MAX_HTML: usize = 1024 * 1024;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!(
                "Mozilla/5.0 (compatible; Kawruh/",
                env!("CARGO_PKG_VERSION"),
                "; link preview)"
            ))
            .build()
            .expect("http client")
    })
}

pub async fn fetch(url: &str) -> Result<LinkMeta> {
    let url = Url::parse(url.trim()).map_err(|_| Error::msg("not a link"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::msg("only http and https links have previews"));
    }
    let mut res = client()
        .get(url.clone())
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,*/*;q=0.8",
        )
        .send()
        .await?;
    let base = res.url().clone();
    let site = host_name(&base);
    let kind = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if kind.starts_with("image/") {
        let title = base
            .path_segments()
            .and_then(|mut s| s.next_back())
            .map(|s| {
                percent_encoding::percent_decode_str(s)
                    .decode_utf8_lossy()
                    .to_string()
            })
            .unwrap_or_default();
        return Ok(LinkMeta {
            title,
            image: url.to_string(),
            site,
            ..LinkMeta::default()
        });
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() >= MAX_HTML {
            break;
        }
    }
    let mut meta = parse(&String::from_utf8_lossy(&body), &base);
    if meta.site.is_empty() {
        meta.site = site;
    }
    Ok(meta)
}

fn host_name(url: &Url) -> String {
    url.host_str()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_string()
}

static META: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<meta\b[^>]*>").unwrap());
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<link\b[^>]*>").unwrap());
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<title\b[^>]*>(.*?)</title>").unwrap());
static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([a-zA-Z_:.-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).unwrap()
});

fn attrs(tag: &str) -> Vec<(String, String)> {
    ATTR.captures_iter(tag)
        .map(|c| {
            let value = c
                .get(2)
                .or(c.get(3))
                .or(c.get(4))
                .map_or("", |m| m.as_str());
            (c[1].to_ascii_lowercase(), value.to_string())
        })
        .collect()
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Reads card metadata from a page's HTML. `base` resolves relative URLs.
pub fn parse(html: &str, base: &Url) -> LinkMeta {
    // Everything a card needs is in <head>; don't scan megabytes of body.
    let head = match html.to_ascii_lowercase().find("</head>") {
        Some(end) => &html[..end],
        None => html,
    };
    let mut props: Vec<(String, String)> = Vec::new();
    for tag in META.find_iter(head) {
        let a = attrs(tag.as_str());
        let key = attr(&a, "property").or(attr(&a, "name"));
        if let (Some(key), Some(content)) = (key, attr(&a, "content")) {
            props.push((key.to_ascii_lowercase(), clean(content)));
        }
    }
    let first = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| props.iter().find(|(p, v)| p == k && !v.is_empty()))
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let resolve = |href: String| -> String {
        base.join(&href)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
            .map(|u| u.to_string())
            .unwrap_or_default()
    };

    let mut title = first(&["og:title", "twitter:title"]);
    if title.is_empty() {
        title = TITLE
            .captures(head)
            .map(|c| clean(&c[1]))
            .unwrap_or_default();
    }
    let image = first(&[
        "og:image:secure_url",
        "og:image",
        "twitter:image",
        "twitter:image:src",
    ]);
    let mut icon = String::new();
    for tag in LINK.find_iter(head) {
        let a = attrs(tag.as_str());
        let rel = attr(&a, "rel").unwrap_or("").to_ascii_lowercase();
        if rel.split_whitespace().any(|r| r == "icon")
            && let Some(href) = attr(&a, "href")
        {
            icon = clean(href);
            break;
        }
    }
    if icon.is_empty() {
        icon = "/favicon.ico".into();
    }
    LinkMeta {
        title: truncate(title, 300),
        description: truncate(
            first(&["og:description", "twitter:description", "description"]),
            500,
        ),
        image: if image.is_empty() {
            image
        } else {
            resolve(image)
        },
        icon: resolve(icon),
        site: first(&["og:site_name"]),
    }
}

fn truncate(s: String, max: usize) -> String {
    if s.chars().count() <= max {
        return s;
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Decodes entities and collapses whitespace.
fn clean(s: &str) -> String {
    decode_entities(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://www.example.com/blog/post").unwrap()
    }

    #[test]
    fn reads_open_graph_first() {
        let html = r#"<html><head>
            <title>Fallback</title>
            <meta property="og:title" content="Kawruh &amp; friends">
            <meta content='A   notebook&#39;s story' property='og:description'>
            <meta name="twitter:image" content="/img/card.png">
            <meta property="og:site_name" content="Example Blog">
            <link rel="shortcut icon" href="/favicon-32.png">
            </head><body><meta property="og:title" content="ignored"></body></html>"#;
        let m = parse(html, &base());
        assert_eq!(m.title, "Kawruh & friends");
        assert_eq!(m.description, "A notebook's story");
        assert_eq!(m.image, "https://www.example.com/img/card.png");
        assert_eq!(m.icon, "https://www.example.com/favicon-32.png");
        assert_eq!(m.site, "Example Blog");
    }

    #[test]
    fn falls_back_to_title_tag_and_default_icon() {
        let m = parse(
            "<head><TITLE>\n  Plain &#x26; simple </TITLE><meta name=description content=Hi></head>",
            &base(),
        );
        assert_eq!(m.title, "Plain & simple");
        assert_eq!(m.description, "Hi");
        assert_eq!(m.icon, "https://www.example.com/favicon.ico");
        assert!(m.image.is_empty());
    }

    #[test]
    fn drops_non_http_urls() {
        let m = parse(
            r#"<head><meta property="og:image" content="javascript:alert(1)"></head>"#,
            &base(),
        );
        assert!(m.image.is_empty());
    }

    #[test]
    fn keeps_unknown_entities_and_stray_ampersands() {
        assert_eq!(decode_entities("a & b &bogus; &#65;"), "a & b &bogus; A");
    }
}
