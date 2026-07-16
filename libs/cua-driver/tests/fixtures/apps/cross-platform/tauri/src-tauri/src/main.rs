#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::thread;

const FIXTURE_HTML: &[u8] = include_bytes!("../../web/index.html");

fn fixture_url() -> tauri::Url {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind Tauri fixture server");
    let address = server
        .server_addr()
        .to_ip()
        .expect("Tauri fixture server must use an IP address");
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let content_type =
                tiny_http::Header::from_bytes(b"Content-Type", b"text/html; charset=utf-8")
                    .expect("valid fixture response header");
            let response = tiny_http::Response::from_data(FIXTURE_HTML).with_header(content_type);
            let _ = request.respond(response);
        }
    });

    tauri::Url::parse(&format!("http://{address}/")).expect("valid Tauri fixture URL")
}

fn main() {
    let journal_url = std::env::var("CUA_E2E_FIXTURE_JOURNAL_URL").unwrap_or_default();
    let journal_plugin = tauri::plugin::Builder::<tauri::Wry, ()>::new("e2e-journal")
        .js_init_script(format!(
            "window.__CUA_E2E_FIXTURE_JOURNAL_URL = {journal_url:?};"
        ))
        .build();
    tauri::Builder::default()
        .plugin(journal_plugin)
        .setup(|app| {
            #[cfg(target_os = "windows")]
            let height = 640.0;
            #[cfg(not(target_os = "windows"))]
            let height = 760.0;

            let window = tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(fixture_url()),
            )
            .title("CuaTestHarness Tauri")
            .inner_size(900.0, height)
            .resizable(true);
            #[cfg(target_os = "windows")]
            let window = window.position(40.0, 40.0);

            window.build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running CuaTestHarness.Tauri");
}
