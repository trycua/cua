//! The native library of the cua SDK, `libcua_sdk`.
//!
//! `cua-sdk` holds the SDK and its UniFFI scaffolding as an rlib, so
//! cargo can start the crates that depend on it before it is fully built (a
//! crate that also builds a cdylib or staticlib needs all of its
//! dependencies complete, and its dependents wait for it complete). This
//! crate only links it into the shared and static libraries the language
//! bindings load; the exported `uniffi_cua_sdk_*` symbols come from the rlib.

pub use cua_sdk::*;
