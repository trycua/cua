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
        .run(tauri::generate_context!())
        .expect("error while running CuaTestHarness.Tauri");
}
