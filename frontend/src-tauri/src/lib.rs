use tauri::Manager;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .invoke_handler(tauri::generate_handler![
      api_base,
      write_text_file,
      read_text_file
    ])
    .setup(|app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }

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

      tauri::async_runtime::spawn(async move {
        if let Err(err) = osp_server::serve(listener).await {
          log::error!("embedded server stopped: {err}");
        }
      });

      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
