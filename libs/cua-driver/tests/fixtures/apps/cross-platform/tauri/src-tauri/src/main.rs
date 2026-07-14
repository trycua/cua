#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let journal_url = std::env::var("CUA_E2E_FIXTURE_JOURNAL_URL").unwrap_or_default();
    let journal_plugin = tauri::plugin::Builder::<tauri::Wry, ()>::new("e2e-journal")
        .js_init_script(format!(
            "window.__CUA_E2E_FIXTURE_JOURNAL_URL = {journal_url:?};"
        ))
        .build();
    tauri::Builder::default()
        .plugin(journal_plugin)
        .setup(|_app| {
            #[cfg(target_os = "windows")]
            {
                use tauri::{LogicalSize, Manager, Size};

                _app.get_webview_window("main")
                    .expect("Tauri main fixture window")
                    .set_size(Size::Logical(LogicalSize::new(900.0, 640.0)))?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running CuaTestHarness.Tauri");
}
