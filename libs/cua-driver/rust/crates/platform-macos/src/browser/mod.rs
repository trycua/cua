pub mod ax_page_reader;
pub mod browser_js;
pub mod cdp_client;
mod consent_ui;
pub mod electron_js;
pub mod platform;
mod setup_ui;
pub mod wk_web_view;

pub use ax_page_reader::AXPageReader;
pub use browser_js::BrowserJs;
pub use cdp_client::{CdpClient, CdpSessionCache};
pub use electron_js::ElectronJs;
pub use platform::MacOsBrowserPlatform;
pub use wk_web_view::is_wk_web_view_app;
