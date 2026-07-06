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
    .invoke_handler(tauri::generate_handler![write_text_file, read_text_file])
    .setup(|app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
