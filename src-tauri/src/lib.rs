pub mod platform;
pub mod plugin;
mod reporting;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .manage(reporting::Reporting::default())
    .invoke_handler(tauri::generate_handler![
      reporting::reporting_load,
      reporting::reporting_save,
      reporting::reporting_select,
      reporting::reporting_snapshot,
      reporting::reporting_export,
      reporting::reporting_release,
      reporting::reporting_start,
      reporting::reporting_stop,
      reporting::reporting_status,
      reporting::reporting_audio,
    ])
    .setup(|app| {
      if cfg!(debug_assertions)
        || std::env::var_os("EVE_ALERT_CAPTURE_DIAGNOSTICS").is_some_and(|value| value == "1")
      {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .filter(|metadata| {
              cfg!(debug_assertions)
                || metadata
                  .target()
                  .starts_with("app_lib::platform::screencast::pipewire")
            })
            .build(),
        )?;
      }
      Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
