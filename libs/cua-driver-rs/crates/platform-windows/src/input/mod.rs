//! Background input synthesis for Windows.
//!
//! Strategy (matches CuaDriver.Win reference):
//! - Mouse: PostMessageW(WM_LBUTTONDOWN/UP) with packed LPARAM(x,y).
//!   PostMessage is async and does NOT activate the target window.
//! - Keyboard text: PostMessageW(WM_CHAR) per character.
//! - Key press: PostMessageW(WM_KEYDOWN) + PostMessageW(WM_KEYUP).
//!
//! For clicks, coordinates are window-client-relative (ScreenToClient applied
//! if screen coords are given). We pack with MAKELPARAM(x,y).

pub mod mouse;
pub mod keyboard;

pub use mouse::{post_click, post_click_screen};
pub use keyboard::{post_char, post_key, post_type_text, post_type_text_with_delay};
