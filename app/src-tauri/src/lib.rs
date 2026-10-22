mod assets;
mod commands;
mod db;
#[cfg(debug_assertions)]
mod e2e;
mod error;
mod history;
mod hub;
mod linkmeta;
mod opened;
mod pdf;
mod remote;
mod search;
mod sync;
mod transfer;

use std::sync::{Arc, Mutex};

use kawruh_core::{store, workspace};
use tauri::Manager;
use tokio::sync::mpsc;

use crate::commands::AppState;
use crate::hub::{DocHub, HubEvent};
use crate::sync::SyncManager;

/// Routes every document update: persist + other views (hub), then the server.
async fn event_loop(
    app: tauri::AppHandle,
    hub: Arc<DocHub>,
    sync: Arc<SyncManager>,
    search: Arc<search::SearchIndex>,
    history: Arc<history::History>,
    mut rx: mpsc::UnboundedReceiver<HubEvent>,
) {
    use tauri::Emitter;
    while let Some(ev) = rx.recv().await {
        let routed = hub.route(&ev);
        search.mark(&ev.doc_id);
        if ev.doc_id != ev.workspace_id {
            snapshot_if_due(&hub, &history, &ev.workspace_id, &ev.doc_id);
        }
        if routed.to_server {
            sync.send_update(&ev.workspace_id, &ev.doc_id, ev.update.clone());
        }
        if let Some(message) = routed.persist_error {
            let _ = app.emit("persist-error", message);
        }
        if ev.doc_id == ev.workspace_id {
            if let Some(doc) = hub.doc(&ev.doc_id) {
                let name = workspace::workspace_name(&doc);
                if !name.is_empty() {
                    let _ = db::set_workspace_name(&hub.db.lock().unwrap(), &ev.doc_id, &name);
                }
            }
            // New pages in the tree start syncing as soon as they appear.
            sync.subscribe_all(&ev.workspace_id);
        }
    }
}

/// Saves a page history snapshot when the page's last one is old enough.
pub(crate) fn snapshot_if_due(
    hub: &DocHub,
    history: &history::History,
    workspace_id: &str,
    page_id: &str,
) {
    let now = workspace::now_ms();
    let due = history.due(&hub.db.lock().unwrap(), page_id, now);
    if !matches!(due, Ok(true)) {
        return;
    }
    let Ok(doc) = hub.get_or_load(page_id, workspace_id) else {
        return;
    };
    let markdown = kawruh_core::markdown::doc_to_markdown(&doc);
    if let Err(e) = history.snapshot(&hub.db.lock().unwrap(), page_id, &markdown, now) {
        tracing::warn!("page history for {page_id}: {e}");
    }
}

fn data_dir(app: &tauri::App) -> std::path::PathBuf {
    match std::env::var_os("KAWRUH_DATA_DIR") {
        Some(dir) => dir.into(),
        None => app
            .path()
            .app_data_dir()
            .expect("no app data directory on this platform"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,kawruh_lib=debug,kawruh_core=debug".into()),
        )
        .try_init();

    let builder = tauri::Builder::default()
        // First, so a second launch (e.g. opening a .md file) hands its
        // arguments to the running app instead of starting another one.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            opened::queue(app, opened::from_args(&argv));
        }))
        .manage(opened::Opened::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .register_asynchronous_uri_scheme_protocol(assets::SCHEME, |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            let path = request.uri().path().to_string();
            tauri::async_runtime::spawn(async move {
                responder.respond(assets::serve(&app, &path).await);
            });
        })
        .setup(|app| {
            let dir = data_dir(app);
            std::fs::create_dir_all(&dir)?;
            tracing::info!("data dir: {}", dir.display());
            let conn = store::open(&dir.join("kawruh.db"))?;
            db::migrate(&conn)?;
            let db = Arc::new(Mutex::new(conn));
            let (hub, rx) = DocHub::new(db);
            let sync = SyncManager::new(hub.clone(), app.handle().clone());
            let search = Arc::new(search::SearchIndex::default());
            let history = Arc::new(history::History::default());
            tauri::async_runtime::spawn(event_loop(
                app.handle().clone(),
                hub.clone(),
                sync.clone(),
                search.clone(),
                history.clone(),
                rx,
            ));
            app.manage(AppState {
                hub,
                sync,
                search,
                history,
                remote: remote::Remote::new(),
                data_dir: dir,
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = handle.state::<AppState>().start_all_syncs() {
                    tracing::error!("starting sync failed: {e}");
                }
            });
            #[cfg(debug_assertions)]
            e2e::start(app.handle().clone());
            // Windows and Linux pass files opened with Kawruh as arguments.
            let args: Vec<String> = std::env::args().collect();
            opened::queue(app.handle(), opened::from_args(&args));
            Ok(())
        });

    #[cfg(debug_assertions)]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::app_info,
        commands::workspaces_list,
        commands::workspace_create,
        commands::workspace_forget,
        commands::workspace_publish,
        commands::workspace_attach,
        commands::remote_workspaces,
        commands::accounts_list,
        commands::account_login,
        commands::account_logout,
        commands::members_list,
        commands::member_add,
        commands::member_remove,
        commands::shares_list,
        commands::share_create,
        commands::share_delete,
        commands::tokens_list,
        commands::notifications_list,
        commands::notifications_read,
        commands::invite_create,
        commands::token_create,
        commands::token_delete,
        commands::doc_open,
        commands::doc_close,
        commands::doc_apply,
        commands::awareness_send,
        commands::page_export_markdown,
        commands::page_import_markdown,
        commands::page_export_file,
        commands::page_import_file,
        commands::workspace_import,
        commands::workspace_export,
        commands::page_print_data,
        commands::search_pages,
        commands::page_backlinks,
        commands::workspace_graph,
        commands::page_history,
        commands::page_history_get,
        commands::page_history_restore,
        commands::open_pending_files,
        commands::pending_files,
        commands::page_export_pdf,
        commands::asset_put,
        commands::link_preview,
        commands::setting_get,
        commands::setting_set,
        e2e::e2e_result,
    ]);
    #[cfg(not(debug_assertions))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        commands::app_info,
        commands::workspaces_list,
        commands::workspace_create,
        commands::workspace_forget,
        commands::workspace_publish,
        commands::workspace_attach,
        commands::remote_workspaces,
        commands::accounts_list,
        commands::account_login,
        commands::account_logout,
        commands::members_list,
        commands::member_add,
        commands::member_remove,
        commands::shares_list,
        commands::share_create,
        commands::share_delete,
        commands::tokens_list,
        commands::notifications_list,
        commands::notifications_read,
        commands::invite_create,
        commands::token_create,
        commands::token_delete,
        commands::doc_open,
        commands::doc_close,
        commands::doc_apply,
        commands::awareness_send,
        commands::page_export_markdown,
        commands::page_import_markdown,
        commands::page_export_file,
        commands::page_import_file,
        commands::workspace_import,
        commands::workspace_export,
        commands::page_print_data,
        commands::search_pages,
        commands::page_backlinks,
        commands::workspace_graph,
        commands::page_history,
        commands::page_history_get,
        commands::page_history_restore,
        commands::open_pending_files,
        commands::pending_files,
        commands::page_export_pdf,
        commands::asset_put,
        commands::link_preview,
        commands::setting_get,
        commands::setting_set,
    ]);

    builder
        .build(tauri::generate_context!())
        .expect("error while building Kawruh")
        .run(|app, event| {
            // macOS hands over files opened with Kawruh as an event instead.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = event {
                opened::queue(app, urls.into_iter().filter_map(|u| u.to_file_path().ok()));
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}
