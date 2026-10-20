//! Markdown files opened with Kawruh (Finder's "Open With", a double-click when
//! Kawruh is the default, or a path on the command line). They wait here until
//! the webview asks to import them into its current workspace, so the webview
//! never names a path on disk itself.

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct Opened(Mutex<Vec<PathBuf>>);

fn is_markdown(path: &std::path::Path) -> bool {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    matches!(ext.as_str(), "md" | "markdown" | "mdown") && path.is_file()
}

/// Paths among command-line arguments (skipping the program and flags).
pub fn from_args(args: &[String]) -> Vec<PathBuf> {
    args.iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect()
}

/// Queues the markdown files among `paths`, tells the webview, and brings the
/// window forward.
pub fn queue(app: &AppHandle, paths: impl IntoIterator<Item = PathBuf>) {
    let files: Vec<PathBuf> = paths.into_iter().filter(|p| is_markdown(p)).collect();
    if files.is_empty() {
        return;
    }
    app.state::<Opened>().0.lock().unwrap().extend(files);
    let _ = app.emit("files-opened", ());
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn count(app: &AppHandle) -> usize {
    app.state::<Opened>().0.lock().unwrap().len()
}

/// Takes every queued file.
pub fn take(app: &AppHandle) -> Vec<PathBuf> {
    std::mem::take(&mut *app.state::<Opened>().0.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_existing_markdown_files_count() {
        let dir = std::env::temp_dir().join(format!("kawruh-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("Notes.MD");
        std::fs::write(&md, "# hi").unwrap();
        let txt = dir.join("notes.txt");
        std::fs::write(&txt, "hi").unwrap();
        assert!(is_markdown(&md));
        assert!(!is_markdown(&txt));
        assert!(!is_markdown(&dir.join("missing.md")));
        std::fs::remove_dir_all(&dir).unwrap();
        let args = vec!["kawruh".into(), "--flag".into(), "/tmp/a.md".into()];
        assert_eq!(from_args(&args), vec![PathBuf::from("/tmp/a.md")]);
    }
}
