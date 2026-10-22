//! A workspace in and out as a folder of markdown files. Import reads an
//! Obsidian vault or a Notion export (a folder, or the .zip Notion sends);
//! export writes the page tree as folders. The commands are in commands.rs.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use kawruh_core::markdown;
use kawruh_core::workspace::{self, PageMeta};
use kawruh_core::yrs::Doc;
use regex::Regex;
use serde::Serialize;

use crate::assets;
use crate::commands::file_name;
use crate::error::{Error, Result};
use crate::hub::ORIGIN_LOCAL;

/// How pages link to each other: `[Title](kawruh://page/<page_id>)`.
pub const PAGE_LINK: &str = "kawruh://page/";
/// The folder of an export that holds every page's images.
pub const ASSETS_DIR: &str = "_assets";
/// Most a zip may unpack to.
const MAX_UNZIPPED: u64 = 1024 * 1024 * 1024;
/// Obsidian embeds of these become images; other embeds become page links.
const IMAGE_EXTS: [&str; 7] = ["png", "jpg", "jpeg", "gif", "webp", "avif", "svg"];

fn io(e: impl std::fmt::Display) -> Error {
    Error::msg(e.to_string())
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn is_markdown(path: &Path) -> bool {
    has_ext(path, &["md", "markdown"])
}

// -------------------------------------------------------------------- import

/// What an import created.
#[derive(Debug, Serialize)]
pub struct Imported {
    pub pages: usize,
    /// The first page created.
    pub root: Option<String>,
}

/// A page to create: a markdown file, a folder, or both (Notion exports a page
/// with sub-pages as `Name.md` beside a `Name/` folder holding them).
struct Entry {
    title: String,
    file: Option<PathBuf>,
    children: Vec<Entry>,
}

/// Where links between imported files lead, filled in as pages are created.
#[derive(Default)]
struct Links {
    /// Markdown file → page id.
    by_path: HashMap<PathBuf, String>,
    /// Lower-case title → page id, for wikilinks. The first page wins.
    by_title: HashMap<String, String>,
    /// Lower-case file name → file, for Obsidian embeds.
    files: HashMap<String, PathBuf>,
}

/// Imports a folder, or a zip unpacked into a temporary folder first, as pages
/// under `parent` ("" = top level) of the workspace doc `ws`. All pages are
/// created before any content so links between files can point at them; then
/// `fill(page, dir, markdown)` sets each page's content, `dir` being the folder
/// its relative image paths start from.
pub fn import(
    ws: &Doc,
    parent: &str,
    path: &Path,
    fill: impl FnMut(&str, &Path, &str) -> Result<()>,
) -> Result<Imported> {
    if !path.is_file() {
        return import_folder(ws, parent, path, fill);
    }
    let dir = std::env::temp_dir().join(format!("kawruh-import-{}", workspace::new_id()));
    let result = unzip(path, &dir).and_then(|()| import_folder(ws, parent, &dir, fill));
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn import_folder(
    ws: &Doc,
    parent: &str,
    root: &Path,
    mut fill: impl FnMut(&str, &Path, &str) -> Result<()>,
) -> Result<Imported> {
    let mut links = Links::default();
    let entries = scan(root, &mut links.files)?;
    if entries.is_empty() {
        return Err(Error::msg("No markdown files found there."));
    }
    let mut created = Vec::new();
    create(ws, parent, &entries, &mut links, &mut created)?;
    for (id, title, file) in &created {
        let Some(file) = file else { continue };
        let text = String::from_utf8_lossy(&std::fs::read(file).map_err(io)?).to_string();
        let dir = file.parent().unwrap_or(root);
        fill(id, dir, &page_markdown(&text, title, dir, &links))?;
    }
    Ok(Imported {
        pages: created.len(),
        root: created.first().map(|(id, ..)| id.clone()),
    })
}

/// The pages in `dir`, by name. Hidden files and folders (`.obsidian`, `.git`)
/// are skipped, and so are folders without markdown in them (Notion keeps a
/// page's images in one). Other files go into `files` for Obsidian embeds.
fn scan(dir: &Path, files: &mut HashMap<String, PathBuf>) -> Result<Vec<Entry>> {
    let mut items = Vec::new();
    for e in std::fs::read_dir(dir).map_err(io)? {
        let e = e.map_err(io)?;
        let name = e.file_name().to_string_lossy().to_string();
        // Symlinks are neither, so a link loop can't trap the walk.
        let kind = e.file_type().map_err(io)?;
        if !name.starts_with('.') && (kind.is_dir() || kind.is_file()) {
            items.push((name, e.path(), kind.is_dir()));
        }
    }
    items.sort_by_key(|(name, ..)| name.to_lowercase());
    let stem_of = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_string());
    let mut out = Vec::new();
    for (name, path, is_dir) in &items {
        if *is_dir {
            let children = scan(path, files)?;
            let file = items
                .iter()
                .find(|(_, p, d)| !d && is_markdown(p) && stem_of(p).as_ref() == Some(name))
                .map(|(_, p, _)| p.clone());
            if children.is_empty() && file.is_none() {
                continue;
            }
            out.push(Entry {
                title: clean_title(name),
                file,
                children,
            });
        } else if is_markdown(path) {
            let stem = stem_of(path).unwrap_or_default();
            // The page of the folder with the same name.
            if items.iter().any(|(n, _, d)| *d && *n == stem) {
                continue;
            }
            out.push(Entry {
                title: clean_title(&stem),
                file: Some(path.clone()),
                children: Vec::new(),
            });
        } else {
            files
                .entry(name.to_lowercase())
                .or_insert_with(|| path.clone());
        }
    }
    Ok(out)
}

/// A file or folder name as a page title, without the 32-hex id Notion appends.
fn clean_title(name: &str) -> String {
    let b = name.as_bytes();
    let n = b.len();
    if n > 33 && b[n - 33] == b' ' && b[n - 32..].iter().all(u8::is_ascii_hexdigit) {
        return name[..n - 33].trim().to_string();
    }
    name.trim().to_string()
}

/// Creates the pages depth-first, collecting (id, title, file) for each.
fn create(
    ws: &Doc,
    parent: &str,
    entries: &[Entry],
    links: &mut Links,
    out: &mut Vec<(String, String, Option<PathBuf>)>,
) -> Result<()> {
    for e in entries {
        let id = workspace::create_page(ws, &e.title, parent, ORIGIN_LOCAL)?;
        links
            .by_title
            .entry(e.title.to_lowercase())
            .or_insert_with(|| id.clone());
        if let Some(file) = &e.file {
            links.by_path.insert(normalize(file), id.clone());
        }
        out.push((id.clone(), e.title.clone(), e.file.clone()));
        create(ws, &id, &e.children, links, out)?;
    }
    Ok(())
}

/// A file's markdown as page content: front matter and a leading `# Title`
/// repeating the page title go, Obsidian embeds and wikilinks are converted,
/// and relative links to imported files (Notion's) become page links.
fn page_markdown(text: &str, title: &str, dir: &Path, links: &Links) -> String {
    let body = strip_title(title, strip_front_matter(text));
    let body = wikilinks(body, dir, links);
    markdown::map_link_hrefs(&body, |href| {
        if href.contains("://") {
            return None;
        }
        let file = href.split('#').next().filter(|f| !f.is_empty())?;
        let rel = percent_encoding::percent_decode_str(file).decode_utf8_lossy();
        let id = links.by_path.get(&normalize(&dir.join(rel.as_ref())))?;
        Some(format!("{PAGE_LINK}{id}"))
    })
}

/// Drops YAML front matter (Obsidian properties), which isn't page content.
fn strip_front_matter(md: &str) -> &str {
    let Some(rest) = md
        .strip_prefix("---\n")
        .or_else(|| md.strip_prefix("---\r\n"))
    else {
        return md;
    };
    let mut end = 0;
    for line in rest.split_inclusive('\n') {
        end += line.len();
        if line.trim_end() == "---" {
            return &rest[end..];
        }
    }
    md
}

/// Drops a leading `# Title` that repeats the page title (Notion starts every
/// file with one), like split_title does.
fn strip_title<'a>(title: &str, md: &'a str) -> &'a str {
    let trimmed = md.trim_start();
    if let Some(rest) = trimmed.strip_prefix("# ") {
        let (line, body) = rest.split_once('\n').unwrap_or((rest, ""));
        if line.trim().to_lowercase() == title.to_lowercase() {
            return body.trim_start_matches(['\r', '\n']);
        }
    }
    md
}

static WIKILINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(!?)\[\[([^\[\]\n]+)\]\]").unwrap());

/// Obsidian's `![[image.png|300]]` becomes an image by relative path, the file
/// found by name anywhere in the vault; `[[Note]]`, `[[Note|Alias]]` and
/// `[[Note#Heading]]` become page links, or plain text when no page has that title.
fn wikilinks(md: &str, dir: &Path, links: &Links) -> String {
    outside_code(md, |text| {
        WIKILINK
            .replace_all(text, |c: &regex::Captures| {
                // Inside tables the pipe is escaped: `[[Note\|Alias]]`.
                let (target, alias) = match c[2].split_once('|') {
                    Some((t, a)) => (t.trim_end_matches('\\').trim(), a.trim()),
                    None => (c[2].trim(), ""),
                };
                let (note, heading) = target.split_once('#').unwrap_or((target, ""));
                let name = note.rsplit('/').next().unwrap_or(note).trim();
                if &c[1] == "!" && has_ext(Path::new(name), &IMAGE_EXTS) {
                    let src = match links.files.get(&name.to_lowercase()) {
                        Some(file) => rel_path(dir, file),
                        None => note.to_string(),
                    };
                    return format!("![](<{src}>)");
                }
                let name = name.strip_suffix(".md").unwrap_or(name);
                let label = match (alias, name) {
                    ("", "") => heading.trim(),
                    ("", name) => name,
                    (alias, _) => alias,
                };
                match links.by_title.get(&name.to_lowercase()) {
                    Some(id) if !name.is_empty() => {
                        format!("[{}]({PAGE_LINK}{id})", escape(label))
                    }
                    _ => escape(label),
                }
            })
            .into_owned()
    })
}

/// Applies `f` to the parts of `md` outside fenced code blocks and inline code.
fn outside_code(md: &str, mut f: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(md.len());
    let mut fence: Option<&str> = None;
    for line in md.split_inclusive('\n') {
        let t = line.trim_start();
        if let Some(open) = fence {
            if t.starts_with(open) {
                fence = None;
            }
            out.push_str(line);
        } else if t.starts_with("```") || t.starts_with("~~~") {
            fence = Some(&t[..3]);
            out.push_str(line);
        } else {
            for (i, part) in line.split('`').enumerate() {
                if i > 0 {
                    out.push('`');
                }
                if i % 2 == 0 {
                    out.push_str(&f(part));
                } else {
                    out.push_str(part);
                }
            }
        }
    }
    out
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// `path` with `.` and `..` resolved, without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// `to` relative to the folder `from`, with `/` separators.
fn rel_path(from: &Path, to: &Path) -> String {
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    std::iter::repeat_n("..".to_string(), from.len() - common)
        .chain(
            to[common..]
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().to_string()),
        )
        .collect::<Vec<_>>()
        .join("/")
}

/// Unpacks a zip into `dest`, skipping entries that would land outside it and
/// stopping at `MAX_UNZIPPED` bytes. Zips at its top level are unpacked into
/// it as well: Notion splits big exports into parts.
pub fn unzip(zip: &Path, dest: &Path) -> Result<()> {
    let mut total = 0;
    extract(zip, dest, &mut total)?;
    let parts: Vec<PathBuf> = std::fs::read_dir(dest)
        .map_err(io)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && has_ext(p, &["zip"]))
        .collect();
    for part in parts {
        extract(&part, dest, &mut total)?;
        std::fs::remove_file(&part).map_err(io)?;
    }
    Ok(())
}

fn extract(zip: &Path, dest: &Path, total: &mut u64) -> Result<()> {
    let bad = |e: zip::result::ZipError| Error::msg(format!("Can't read {}: {e}", zip.display()));
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip).map_err(io)?).map_err(bad)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(bad)?;
        // None for absolute paths and ones that climb out with `..`.
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(io)?;
            continue;
        }
        if !entry.is_file() {
            continue;
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let mut file = std::fs::File::create(&out).map_err(io)?;
        let left = MAX_UNZIPPED + 1 - *total;
        *total += std::io::copy(&mut entry.by_ref().take(left), &mut file).map_err(io)?;
        if *total > MAX_UNZIPPED {
            return Err(Error::msg("That zip unpacks to more than 1 GB."));
        }
    }
    Ok(())
}

// -------------------------------------------------------------------- export

/// `name`, or the first of `name (2)`, `name (3)`, … that isn't `taken`.
pub fn dedupe(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut out = name.to_string();
    let mut n = 1;
    while taken(&out) {
        n += 1;
        out = format!("{name} ({n})");
    }
    out
}

/// Images a page keeps in the asset store, as (workspace, name).
pub fn asset_refs(md: &str) -> Vec<(String, String)> {
    let mut refs = Vec::new();
    markdown::map_image_srcs(md, |src| {
        if let Some((ws, name)) = assets::parse_src(src) {
            refs.push((ws.to_string(), name.to_string()));
        }
        None
    });
    refs
}

/// Writes each live page with its markdown under `root`: `Title.md`, and its
/// sub-pages in `Title/`. Images already `saved` in `_assets/` and links to
/// other pages point at those files by relative path.
pub fn export(root: &Path, pages: &[(PageMeta, String)], saved: &HashSet<String>) -> Result<()> {
    std::fs::create_dir_all(root).map_err(io)?;
    let live: Vec<PageMeta> = pages.iter().map(|(p, _)| p.clone()).collect();
    let paths = export_paths(&live);
    for (page, md) in pages {
        let Some(path) = paths.get(&page.id) else {
            continue;
        };
        let body = export_markdown(md, path, &paths, saved);
        let text = if page.title.is_empty() {
            body
        } else {
            format!("# {}\n\n{body}", page.title)
        };
        let file = root.join(path);
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        std::fs::write(&file, text).map_err(io)?;
    }
    Ok(())
}

/// Where each live page goes, relative to the export folder. Same-named
/// siblings get `Title (2)`, `Title (3)`, …; pages whose parent never synced
/// go at the top, like the sidebar shows them.
fn export_paths(live: &[PageMeta]) -> HashMap<String, PathBuf> {
    fn walk(
        live: &[PageMeta],
        ids: &HashSet<&str>,
        parent: &str,
        dir: &Path,
        out: &mut HashMap<String, PathBuf>,
    ) {
        let mut taken = HashSet::new();
        if parent.is_empty() {
            taken.insert(ASSETS_DIR.to_string());
        }
        let children = live.iter().filter(|p| {
            p.parent == parent || (parent.is_empty() && !ids.contains(p.parent.as_str()))
        });
        for p in children {
            // Case-insensitive, as on macOS and Windows file systems.
            let name = dedupe(&file_name(&p.title), |n| taken.contains(&n.to_lowercase()));
            taken.insert(name.to_lowercase());
            out.insert(p.id.clone(), dir.join(format!("{name}.md")));
            walk(live, ids, &p.id, &dir.join(&name), out);
        }
    }
    let ids = live.iter().map(|p| p.id.as_str()).collect();
    let mut out = HashMap::new();
    walk(live, &ids, "", Path::new(""), &mut out);
    out
}

/// A page's markdown as written to `path`.
fn export_markdown(
    md: &str,
    path: &Path,
    paths: &HashMap<String, PathBuf>,
    saved: &HashSet<String>,
) -> String {
    let dir = path.parent().unwrap_or(Path::new(""));
    let md = markdown::map_image_srcs(md, |src| {
        let (_, name) = assets::parse_src(src)?;
        saved
            .contains(name)
            .then(|| rel_path(dir, &Path::new(ASSETS_DIR).join(name)))
    });
    markdown::map_link_hrefs(&md, |href| {
        let to = paths.get(href.strip_prefix(PAGE_LINK)?)?;
        Some(rel_path(dir, to))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::relink_local_images;
    use std::io::Write;

    const NOTION_ID: &str = "0123456789abcdef0123456789abcdef";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kawruh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// Page id → the markdown it was filled with, and that markdown with its
    /// images stored the way the app does (here as `asset-<byte count>`).
    type Filled = HashMap<String, (String, String)>;

    /// Imports `path` into a fresh workspace doc.
    fn run(path: &Path) -> (Vec<PageMeta>, Filled) {
        let ws = Doc::new();
        let mut filled = HashMap::new();
        let imported = import(&ws, "", path, |page, dir, md| {
            let stored = relink_local_images(md, dir, |b| Some(format!("asset-{}", b.len())));
            filled.insert(page.to_string(), (md.to_string(), stored));
            Ok(())
        })
        .unwrap();
        let live = workspace::live_pages(&ws);
        assert_eq!(imported.pages, live.len());
        let first = live.iter().find(|p| p.parent.is_empty()).unwrap();
        assert_eq!(imported.root.as_ref(), Some(&first.id));
        (live, filled)
    }

    /// Titles in outline order, indented by depth.
    fn tree(live: &[PageMeta], parent: &str, depth: usize) -> Vec<String> {
        let mut out = Vec::new();
        for p in live.iter().filter(|p| p.parent == parent) {
            out.push(format!("{}{}", "  ".repeat(depth), p.title));
            out.extend(tree(live, &p.id, depth + 1));
        }
        out
    }

    fn by_title<'a>(live: &'a [PageMeta], title: &str) -> &'a PageMeta {
        live.iter()
            .find(|p| p.title == title)
            .unwrap_or_else(|| panic!("no page {title}: {live:?}"))
    }

    #[test]
    fn imports_an_obsidian_vault() {
        let vault = temp("obsidian");
        write(&vault, ".obsidian/app.json", b"{}");
        write(&vault, ".trash/Old.md", b"old");
        write(&vault, "attachments/diagram.png", b"png bytes");
        write(&vault, "todo.txt", b"not markdown");
        write(
            &vault,
            "Home.md",
            b"---\ntags: [a]\n---\n# Home\n\nSee [[Projects/Plan|the plan]], [[Ideas#Later]] \
              and [[Missing]].\n\n![[diagram.png|300]]\n\nKeep `[[code]]` as is.\n",
        );
        write(&vault, "Ideas.md", b"Back [[home]]\n");
        write(&vault, "Projects/Plan.md", b"![[Diagram.png]]\n");

        let (live, filled) = run(&vault);
        assert_eq!(tree(&live, "", 0), ["Home", "Ideas", "Projects", "  Plan"]);
        let (home, ideas, projects, plan) = (
            by_title(&live, "Home"),
            by_title(&live, "Ideas"),
            by_title(&live, "Projects"),
            by_title(&live, "Plan"),
        );
        assert!(
            !filled.contains_key(&projects.id),
            "a plain folder is an empty page"
        );

        let (md, stored) = &filled[&home.id];
        assert_eq!(
            md,
            &format!(
                "See [the plan](kawruh://page/{}), [Ideas](kawruh://page/{}) and Missing.\n\n\
                 ![](attachments/diagram.png)\n\nKeep `[[code]]` as is.\n",
                plan.id, ideas.id
            )
        );
        assert!(stored.contains("\n![](asset-9)\n"), "{stored}");
        assert_eq!(
            filled[&ideas.id].0,
            format!("Back [home](kawruh://page/{})\n", home.id)
        );
        assert_eq!(filled[&plan.id].0, "![](../attachments/diagram.png)\n");
        assert_eq!(filled[&plan.id].1, "![](asset-9)\n");
        std::fs::remove_dir_all(&vault).unwrap();
    }

    /// A Notion export: ids on every name, a page with sub-pages as `X.md` +
    /// `X/`, a database as CSV + folder of rows, links between files.
    fn notion_files() -> Vec<(String, Vec<u8>)> {
        let id = NOTION_ID;
        vec![
            (
                format!("Team {id}.md"),
                format!(
                    "# Team\n\nSee [Meeting notes](Team%20{id}/Meeting%20notes%20{id}.md) \
                     and ![chart](Team%20{id}/chart.png)\n"
                )
                .into_bytes(),
            ),
            (format!("Team {id}/chart.png"), b"png bytes".to_vec()),
            (
                format!("Team {id}/Meeting notes {id}.md"),
                format!("# Meeting notes\n\nBack to [Team](../Team%20{id}.md#top)\n").into_bytes(),
            ),
            (
                format!("Team {id}/Tasks {id}.csv"),
                b"Name,Status\n".to_vec(),
            ),
            (
                format!("Team {id}/Tasks {id}/Ship it {id}.md"),
                b"# Ship it\n\nStatus: Done\n".to_vec(),
            ),
        ]
    }

    fn check_notion(live: &[PageMeta], filled: &Filled) {
        assert_eq!(
            tree(live, "", 0),
            ["Team", "  Meeting notes", "  Tasks", "    Ship it"]
        );
        let (team, notes, ship) = (
            by_title(live, "Team"),
            by_title(live, "Meeting notes"),
            by_title(live, "Ship it"),
        );
        assert_eq!(
            filled[&team.id].1,
            format!(
                "See [Meeting notes](kawruh://page/{}) and ![chart](asset-9)\n",
                notes.id
            )
        );
        assert_eq!(
            filled[&notes.id].0,
            format!("Back to [Team](kawruh://page/{})\n", team.id)
        );
        assert_eq!(filled[&ship.id].0, "Status: Done\n");
    }

    #[test]
    fn imports_a_notion_export_folder() {
        let dir = temp("notion");
        for (name, bytes) in notion_files() {
            write(&dir, &name, &bytes);
        }
        let (live, filled) = run(&dir);
        check_notion(&live, &filled);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn zip_bytes(files: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in files {
            w.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn imports_a_notion_zip_split_in_parts() {
        let dir = temp("notion-zip");
        let part = zip_bytes(&notion_files());
        let outer = zip_bytes(&[
            ("Export-Part-1.zip".into(), part),
            ("../escaped.md".into(), b"# Out".to_vec()),
        ]);
        let zip = dir.join("Export.zip");
        std::fs::write(&zip, outer).unwrap();
        let (live, filled) = run(&zip);
        check_notion(&live, &filled);
        assert!(!std::env::temp_dir().join("escaped.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn notion_ids_leave_titles() {
        assert_eq!(
            clean_title(&format!("Meeting notes {NOTION_ID}")),
            "Meeting notes"
        );
        assert_eq!(clean_title("Plain"), "Plain");
        assert_eq!(
            clean_title(&format!("Tail{NOTION_ID}")),
            format!("Tail{NOTION_ID}")
        );
        assert_eq!(strip_title("Home", "# home\n\nbody"), "body");
        assert_eq!(strip_title("Home", "# Other\n\nbody"), "# Other\n\nbody");
    }

    #[test]
    fn exports_folders_assets_and_relative_links() {
        let root = temp("export");
        let page = |id: &str, title: &str, parent: &str, order: f64| PageMeta {
            id: id.into(),
            title: title.into(),
            parent: parent.into(),
            order,
            icon: String::new(),
            created: 0,
            deleted: 0,
            kind: String::new(),
        };
        let image = format!("{}.png", "a".repeat(64));
        let pages = vec![
            (
                page("a", "Plans", "", 1.0),
                format!("To [the list](kawruh://page/b) ![x](kawruh-asset://w/{image})\n"),
            ),
            (
                page("b", "Q4: list", "a", 1.0),
                "Up [Plans](kawruh://page/a), [other](kawruh://page/c) [gone](kawruh://page/zz)\n"
                    .into(),
            ),
            (page("c", "plans", "", 2.0), String::new()),
            (page("d", "_assets", "", 3.0), String::new()),
        ];
        let saved = HashSet::from([image.clone()]);
        export(&root, &pages, &saved).unwrap();
        let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
        assert_eq!(
            read("Plans.md"),
            format!("# Plans\n\nTo [the list](<Plans/Q4- list.md>) ![x](_assets/{image})\n")
        );
        assert_eq!(
            read("Plans/Q4- list.md"),
            "# Q4: list\n\nUp [Plans](../Plans.md), [other](<../plans (2).md>) \
             [gone](kawruh://page/zz)\n"
        );
        assert_eq!(read("plans (2).md"), "# plans\n\n");
        assert_eq!(read("_assets (2).md"), "# _assets\n\n");
        assert_eq!(dedupe("x", |n| n == "x" || n == "x (2)"), "x (3)");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
