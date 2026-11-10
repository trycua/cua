// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! libxkbcommon 1.0 entry points loaded at run time.
//!
//! niri text/key planning needs two functions added in libxkbcommon 1.0. The
//! driver is built against 1.0 but must keep starting on systems that ship an
//! older library (Ubuntu 20.04 has 0.10), where a link-time reference would
//! make the dynamic loader reject the whole binary. Resolve them from the
//! library already loaded at run time instead; a missing symbol only disables
//! niri keyboard planning with a clear error.

use anyhow::{anyhow, Result};
use std::sync::OnceLock;
use xkbcommon::xkb;
use xkbcommon::xkb::ffi as xkb_ffi;

type Utf32ToKeysym = unsafe extern "C" fn(u32) -> u32;
type KeyGetModsForLevel =
    unsafe extern "C" fn(*mut xkb_ffi::xkb_keymap, u32, u32, u32, *mut u32, usize) -> usize;

struct Xkb {
    utf32_to_keysym: Utf32ToKeysym,
    key_get_mods_for_level: KeyGetModsForLevel,
    _library: libloading::Library,
}

fn load() -> Result<Xkb> {
    // SAFETY: libxkbcommon has no load-time initialisers with preconditions.
    unsafe {
        let library = libloading::Library::new("libxkbcommon.so.0")?;
        let utf32_to_keysym = *library.get::<Utf32ToKeysym>(b"xkb_utf32_to_keysym\0")?;
        let key_get_mods_for_level =
            *library.get::<KeyGetModsForLevel>(b"xkb_keymap_key_get_mods_for_level\0")?;
        Ok(Xkb {
            utf32_to_keysym,
            key_get_mods_for_level,
            _library: library,
        })
    }
}

fn runtime() -> Result<&'static Xkb> {
    static LOADED: OnceLock<Result<Xkb>> = OnceLock::new();
    LOADED
        .get_or_init(load)
        .as_ref()
        .map_err(|error| anyhow!("niri keyboard input needs libxkbcommon 1.0 or later: {error}"))
}

pub(super) fn utf32_to_keysym(codepoint: u32) -> Result<xkb::Keysym> {
    // SAFETY: the function pointer was resolved from libxkbcommon with its
    // declared signature and the library stays loaded in `runtime`.
    Ok(xkb::Keysym::new(unsafe {
        (runtime()?.utf32_to_keysym)(codepoint)
    }))
}

pub(super) fn key_get_mods_for_level(
    map: &xkb::Keymap,
    code: xkb::Keycode,
    layout: u32,
    level: u32,
    masks: &mut [u32],
) -> Result<usize> {
    // SAFETY: `map` owns a live xkb_keymap for the call; `masks` is valid for
    // its length; the library stays loaded in `runtime`.
    Ok(unsafe {
        (runtime()?.key_get_mods_for_level)(
            map.get_raw_ptr(),
            code.raw(),
            layout,
            level,
            masks.as_mut_ptr(),
            masks.len(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysym_lookup_works_when_the_runtime_is_available() {
        if runtime().is_err() {
            return;
        }
        assert_eq!(utf32_to_keysym(u32::from('A')).unwrap().raw(), 0x41);
        assert_eq!(utf32_to_keysym(u32::from('😀')).unwrap().raw(), 0x0101_f600);
    }
}
