//! Page → PDF. The webview prints its print-media layout (see `.kw-print` in
//! styles.css). On macOS that goes straight to a paginated PDF file without a
//! print panel; elsewhere the native print dialog opens and its "Save as PDF"
//! printer writes the file.

use std::path::Path;
#[cfg(target_os = "macos")]
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::error::{Error, Result};

/// Writes the main webview's print layout to `path` as a PDF.
#[cfg(target_os = "macos")]
pub async fn print_to_pdf(app: &AppHandle, path: &Path) -> Result<()> {
    use objc2_app_kit::{
        NSPrintInfo, NSPrintJobSavingURL, NSPrintSaveJob, NSPrintingPaginationMode, NSWindow,
    };
    use objc2_foundation::{NSCopying, NSString, NSURL};
    use objc2_web_kit::WKWebView;

    let win = app
        .get_webview_window("main")
        .ok_or_else(|| Error::msg("no main window"))?;
    let _ = std::fs::remove_file(path);
    let target = path.to_string_lossy().to_string();
    win.with_webview(move |wv| unsafe {
        let webview = &*(wv.inner() as *const WKWebView);
        let window = &*(wv.ns_window() as *const NSWindow);
        let info = NSPrintInfo::sharedPrintInfo().copy();
        info.setJobDisposition(NSPrintSaveJob);
        let url = NSURL::fileURLWithPath(&NSString::from_str(&target));
        info.dictionary().insert(NSPrintJobSavingURL, url.as_ref());
        // Half-inch margins; the page content sets its own spacing.
        info.setTopMargin(36.0);
        info.setBottomMargin(36.0);
        info.setLeftMargin(36.0);
        info.setRightMargin(36.0);
        info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
        info.setVerticalPagination(NSPrintingPaginationMode::Automatic);
        info.setHorizontallyCentered(false);
        info.setVerticallyCentered(false);
        let op = webview.printOperationWithPrintInfo(&info);
        op.setShowsPrintPanel(false);
        op.setShowsProgressPanel(false);
        // WebKit lays out the print view itself; it only needs a size to start from.
        if let Some(view) = op.view() {
            view.setFrame(webview.frame());
        }
        op.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            window,
            None,
            None,
            std::ptr::null_mut(),
        );
    })
    .map_err(|e| Error::msg(e.to_string()))?;
    wait_for_file(path).await
}

/// The print operation runs asynchronously and reports nothing back here; the
/// PDF is done once the file exists and has stopped growing.
#[cfg(target_os = "macos")]
async fn wait_for_file(path: &Path) -> Result<()> {
    let mut last = None;
    for _ in 0..120 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let size = std::fs::metadata(path)
            .ok()
            .map(|m| m.len())
            .filter(|n| *n > 0);
        if size.is_some() && size == last {
            return Ok(());
        }
        last = size;
    }
    Err(Error::msg("the PDF wasn't written in time"))
}

/// Opens the native print dialog (choose "Save as PDF" there).
#[cfg(not(target_os = "macos"))]
pub async fn print_to_pdf(app: &AppHandle, _path: &Path) -> Result<()> {
    let win = app
        .get_webview_window("main")
        .ok_or_else(|| Error::msg("no main window"))?;
    win.print().map_err(|e| Error::msg(e.to_string()))
}
