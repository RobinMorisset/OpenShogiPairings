use tauri::Manager;

#[cfg(target_os = "macos")]
mod print_macos;

/// The base URL of the embedded API server, resolved at startup and exposed to
/// the frontend via the [`api_base`] command.
struct ApiBase(String);

/// Return the base URL the frontend should use to reach the embedded server.
///
/// The port is chosen by the OS at startup (see [`run`]), so the frontend must
/// ask for it rather than assuming a fixed port.
#[tauri::command]
fn api_base(state: tauri::State<'_, ApiBase>) -> String {
    state.0.clone()
}

/// Write UTF-8 text to a path chosen by the user via the native save dialog.
///
/// The frontend picks the path with the dialog plugin, then calls this. We use
/// `std::fs` directly (rather than the fs plugin) so that writing to an
/// arbitrary user-selected location doesn't require configuring an fs scope.
#[tauri::command]
fn write_text_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, contents).map_err(|e| e.to_string())
}

/// Read UTF-8 text from a path chosen by the user via the native open dialog.
#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

/// Open the native print dialog for the calling webview window, and answer only
/// once the print is over.
///
/// The webview's JavaScript `window.print()` is a no-op in WKWebView on macOS,
/// so the print buttons must go through this native path instead, which
/// delegates to the platform webview's own print operation. `landscape`
/// requests landscape paper, which that path does not take from the CSS
/// `@page { size: landscape }` the browser build relies on.
///
/// Waiting for the print to *finish* is as much of the contract as starting it:
/// the QR-code and result-sheet buttons print a document they put into the DOM
/// for the occasion and take out again as soon as this returns (see
/// `publication.svelte.ts`), so returning early means printing the ordinary
/// page instead. On macOS that is what [`print_macos::print_and_wait`] is for.
///
/// Elsewhere `WebviewWindow::print` is all there is — on Windows it opens
/// WebView2's print UI and returns just as immediately, with no completion to
/// hook into, so those two buttons keep that race there.
#[tauri::command]
async fn print_window(webview_window: tauri::WebviewWindow, landscape: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let result = print_macos::print_and_wait(webview_window, landscape).await;
    #[cfg(not(target_os = "macos"))]
    let result = {
        let _ = landscape;
        webview_window.print().map_err(|e| e.to_string())
    };
    result
}

/// Where the embedded server persists its tournaments, so the desktop app's
/// registry (and picker) survives a restart — the same per-user data
/// directory `osp-server`'s own automatic backups already use (see
/// `crates/server/src/backup.rs`), just a `tournaments` sibling of `backups`.
/// `None` (no known data directory on this platform) falls back to in-memory
/// only, same as a throwaway dev server.
///
/// `OSP_DATA_DIR` overrides it — the same variable the standalone `osp-server`
/// reads, so one name means one thing across both. It exists for the installs
/// the default doesn't suit: a synced folder, a second drive, a portable
/// install that keeps its data beside the executable.
fn local_data_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var_os("OSP_DATA_DIR") {
        return Some(dir.into());
    }
    Some(
        dirs::data_dir()?
            .join("openshogipairings")
            .join("tournaments"),
    )
}

/// Where the embedded server writes its automatic backups, overridable with
/// `OSP_BACKUP_DIR` (again the standalone server's own variable).
///
/// Separate from [`local_data_dir`] rather than derived from it: the backups
/// are the recovery copy, so "put the tournaments on the synced drive" and
/// "keep the backups on this machine" are both reasonable and neither should
/// imply the other. Left to the server's own default when unset.
fn local_backup_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("OSP_BACKUP_DIR").map(Into::into)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            api_base,
            write_text_file,
            read_text_file,
            print_window
        ])
        .setup(|app| {
            // Log in release too, not just in debug: the packaged app is where a
            // referee actually meets a full disk or a read-only data directory, and
            // the line saying their tournament is no longer being saved ("persist:
            // could not write …") has to land somewhere they can be asked to look.
            // The embedded server logs through `tracing`, which forwards to the
            // `log` facade this plugin captures — no `tracing` subscriber is
            // installed here, which is exactly the condition that forwarding needs.
            //
            // Release logs at WARN, because the default target keeps ONE file capped
            // at 40 KB and *deletes* it when it overflows (`KeepOne`): at INFO the
            // one interesting line would be rotated away by ordinary chatter, while
            // a file that stays near-empty until something breaks holds a whole
            // tournament's worth of trouble. This crate's own startup lines are kept
            // at INFO either way — three lines per run, and the first question after
            // "where did my tournament go?" is which directory it was using.
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(if cfg!(debug_assertions) {
                        log::LevelFilter::Info
                    } else {
                        log::LevelFilter::Warn
                    })
                    .level_for("app_lib", log::LevelFilter::Info)
                    .build(),
            )?;

            // Start the API server in-process. Bind to 127.0.0.1:0 so the OS picks a
            // free port — this avoids clashing with anything else on the machine and
            // is what makes the packaged app run reliably anywhere. We bind
            // synchronously here so the port is known before any window logic runs,
            // then hand the listener to a background task to serve requests.
            let listener = tauri::async_runtime::block_on(async {
                tokio::net::TcpListener::bind(("127.0.0.1", 0)).await
            })?;
            let port = listener.local_addr()?.port();
            let base = format!("http://127.0.0.1:{port}");
            log::info!("OpenShogiPairings embedded server listening on {base}");
            app.manage(ApiBase(base));

            // No admin password and no static dir (the frontend is the webview
            // itself, not served by this API) — just persistence, so tournaments
            // created here survive an app restart. See `local_data_dir`.
            let config = osp_server::ServerConfig {
                data_dir: local_data_dir(),
                backup_dir: local_backup_dir(),
                ..Default::default()
            };
            // Log both resolved locations: they are configurable now, and the first
            // question after "where did my tournament go?" is which directory this
            // app was actually using.
            match &config.data_dir {
                Some(dir) => log::info!("tournaments stored under {}", dir.display()),
                None => log::warn!("no data directory — tournaments live in memory only"),
            }
            match config
                .backup_dir
                .clone()
                .or_else(osp_server::backup_default_root)
            {
                Some(dir) => log::info!("automatic backups written under {}", dir.display()),
                None => log::warn!("no backups directory — automatic backups are disabled"),
            }
            tauri::async_runtime::spawn(async move {
                if let Err(err) = osp_server::serve_with_config(listener, config).await {
                    log::error!("embedded server stopped: {err}");
                }
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
