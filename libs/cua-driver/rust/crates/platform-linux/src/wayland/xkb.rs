// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The part of libxkbcommon that typing uses, declared here and nowhere else.
//!
//! The `xkbcommon` crate also wraps libxkbcommon 1.0 functions. A linker such
//! as mold keeps an import from every object it links, also when the code
//! that uses the import is dropped, so with that crate the binary could need
//! libxkbcommon 1.0 to load at all. This module imports only functions that
//! libxkbcommon 0.5 and 0.6 have. [`ffi`] declares the two 1.0 functions
//! typing needs only for a type check: `text_keys::xkb1` looks them up at run
//! time.

use std::ffi::{c_char, CStr, CString};

pub(crate) use xkeysym::KeyCode as Keycode;
pub(crate) use xkeysym::Keysym;

pub(crate) type ContextFlags = u32;
pub(crate) type KeymapFormat = u32;
pub(crate) type KeymapCompileFlags = u32;
pub(crate) type KeysymFlags = u32;
pub(crate) type StateComponent = u32;
pub(crate) type ModIndex = u32;
pub(crate) type ModMask = u32;
pub(crate) type LayoutIndex = u32;
pub(crate) type LevelIndex = u32;

pub(crate) const CONTEXT_NO_DEFAULT_INCLUDES: ContextFlags = 1 << 0;
pub(crate) const CONTEXT_NO_ENVIRONMENT_NAMES: ContextFlags = 1 << 1;
pub(crate) const KEYMAP_FORMAT_TEXT_V1: KeymapFormat = 1;
pub(crate) const KEYMAP_COMPILE_NO_FLAGS: KeymapCompileFlags = 0;
pub(crate) const KEYSYM_NO_FLAGS: KeysymFlags = 0;
pub(crate) const KEYSYM_CASE_INSENSITIVE: KeysymFlags = 1 << 0;
pub(crate) const STATE_MODS_DEPRESSED: StateComponent = 1 << 0;
pub(crate) const STATE_MODS_LATCHED: StateComponent = 1 << 1;
pub(crate) const STATE_MODS_LOCKED: StateComponent = 1 << 2;
pub(crate) const STATE_MODS_EFFECTIVE: StateComponent = 1 << 3;
pub(crate) const STATE_LAYOUT_EFFECTIVE: StateComponent = 1 << 7;

pub(crate) const MOD_NAME_SHIFT: &str = "Shift";
pub(crate) const MOD_NAME_CAPS: &str = "Lock";
pub(crate) const MOD_NAME_CTRL: &str = "Control";
pub(crate) const MOD_NAME_ALT: &str = "Mod1";
pub(crate) const MOD_NAME_NUM: &str = "Mod2";
pub(crate) const MOD_NAME_LOGO: &str = "Mod4";

/// Which way a key moves (`enum xkb_key_direction`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum KeyDirection {
    Down = 1,
}

pub(crate) mod ffi {
    use std::ffi::{c_char, c_int};

    #[repr(C)]
    pub(crate) struct xkb_context {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub(crate) struct xkb_keymap {
        _private: [u8; 0],
    }
    #[repr(C)]
    pub(crate) struct xkb_state {
        _private: [u8; 0],
    }

    #[link(name = "xkbcommon")]
    extern "C" {
        pub(crate) fn xkb_context_new(flags: u32) -> *mut xkb_context;
        pub(crate) fn xkb_context_unref(context: *mut xkb_context);
        pub(crate) fn xkb_keymap_new_from_buffer(
            context: *mut xkb_context,
            buffer: *const c_char,
            length: usize,
            format: u32,
            flags: u32,
        ) -> *mut xkb_keymap;
        pub(crate) fn xkb_keymap_ref(keymap: *mut xkb_keymap) -> *mut xkb_keymap;
        pub(crate) fn xkb_keymap_unref(keymap: *mut xkb_keymap);
        pub(crate) fn xkb_keymap_min_keycode(keymap: *mut xkb_keymap) -> u32;
        pub(crate) fn xkb_keymap_max_keycode(keymap: *mut xkb_keymap) -> u32;
        pub(crate) fn xkb_keymap_num_mods(keymap: *mut xkb_keymap) -> u32;
        pub(crate) fn xkb_keymap_mod_get_name(keymap: *mut xkb_keymap, idx: u32) -> *const c_char;
        pub(crate) fn xkb_keymap_mod_get_index(keymap: *mut xkb_keymap, name: *const c_char)
            -> u32;
        pub(crate) fn xkb_keymap_num_layouts(keymap: *mut xkb_keymap) -> u32;
        pub(crate) fn xkb_keymap_key_get_name(keymap: *mut xkb_keymap, key: u32) -> *const c_char;
        pub(crate) fn xkb_keymap_num_layouts_for_key(keymap: *mut xkb_keymap, key: u32) -> u32;
        pub(crate) fn xkb_keymap_num_levels_for_key(
            keymap: *mut xkb_keymap,
            key: u32,
            layout: u32,
        ) -> u32;
        pub(crate) fn xkb_keymap_key_get_syms_by_level(
            keymap: *mut xkb_keymap,
            key: u32,
            layout: u32,
            level: u32,
            syms_out: *mut *const u32,
        ) -> c_int;
        pub(crate) fn xkb_state_new(keymap: *mut xkb_keymap) -> *mut xkb_state;
        pub(crate) fn xkb_state_unref(state: *mut xkb_state);
        pub(crate) fn xkb_state_update_key(state: *mut xkb_state, key: u32, direction: u32) -> u32;
        pub(crate) fn xkb_state_update_mask(
            state: *mut xkb_state,
            depressed_mods: u32,
            latched_mods: u32,
            locked_mods: u32,
            depressed_layout: u32,
            latched_layout: u32,
            locked_layout: u32,
        ) -> u32;
        pub(crate) fn xkb_state_key_get_one_sym(state: *mut xkb_state, key: u32) -> u32;
        pub(crate) fn xkb_state_serialize_mods(state: *mut xkb_state, components: u32) -> u32;
        pub(crate) fn xkb_state_serialize_layout(state: *mut xkb_state, components: u32) -> u32;
        pub(crate) fn xkb_keysym_get_name(keysym: u32, buffer: *mut c_char, size: usize) -> c_int;
        pub(crate) fn xkb_keysym_from_name(name: *const c_char, flags: u32) -> u32;

        // libxkbcommon 1.0. Only `text_keys::xkb1` names these, to check the
        // type of the pointers it looks up; nothing calls them through here.
        pub(crate) fn xkb_utf32_to_keysym(ucs: u32) -> u32;
        pub(crate) fn xkb_keymap_key_get_mods_for_level(
            keymap: *mut xkb_keymap,
            key: u32,
            layout: u32,
            level: u32,
            masks_out: *mut u32,
            masks_size: usize,
        ) -> usize;
    }
}

/// A libxkbcommon context.
pub(crate) struct Context {
    ptr: *mut ffi::xkb_context,
}

impl Context {
    pub(crate) fn new(flags: ContextFlags) -> Context {
        // SAFETY: any flags are allowed; an unknown flag gives NULL.
        Context {
            ptr: unsafe { ffi::xkb_context_new(flags) },
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: the pointer came from `xkb_context_new`; NULL is a no-op.
        unsafe { ffi::xkb_context_unref(self.ptr) }
    }
}

/// A compiled keymap.
pub(crate) struct Keymap {
    ptr: *mut ffi::xkb_keymap,
}

// SAFETY: a keymap is immutable after it is compiled, and libxkbcommon counts
// its references atomically.
unsafe impl Send for Keymap {}
// SAFETY: as for `Send`.
unsafe impl Sync for Keymap {}

impl Clone for Keymap {
    fn clone(&self) -> Self {
        // SAFETY: `ptr` is a live keymap.
        Keymap {
            ptr: unsafe { ffi::xkb_keymap_ref(self.ptr) },
        }
    }
}

impl Drop for Keymap {
    fn drop(&mut self) {
        // SAFETY: this value holds one reference to the keymap.
        unsafe { ffi::xkb_keymap_unref(self.ptr) }
    }
}

/// A NUL-terminated string libxkbcommon owns, or `None` for NULL.
///
/// # Safety
/// `ptr` is NULL or a NUL-terminated string that lives at least as long as `'a`.
unsafe fn borrowed<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees a live NUL-terminated string.
    std::str::from_utf8(unsafe { CStr::from_ptr(ptr) }.to_bytes()).ok()
}

impl Keymap {
    /// Compile `string`, or `None` when it is not a keymap.
    pub(crate) fn new_from_string(
        context: &Context,
        string: String,
        format: KeymapFormat,
        flags: KeymapCompileFlags,
    ) -> Option<Keymap> {
        if context.ptr.is_null() {
            return None;
        }
        // SAFETY: the context is live, and the buffer and its length come
        // from one `String`.
        let ptr = unsafe {
            ffi::xkb_keymap_new_from_buffer(
                context.ptr,
                string.as_ptr().cast(),
                string.len(),
                format,
                flags,
            )
        };
        (!ptr.is_null()).then_some(Keymap { ptr })
    }

    pub(crate) fn min_keycode(&self) -> Keycode {
        // SAFETY: `ptr` is a live keymap.
        Keycode::new(unsafe { ffi::xkb_keymap_min_keycode(self.ptr) })
    }

    pub(crate) fn max_keycode(&self) -> Keycode {
        // SAFETY: `ptr` is a live keymap.
        Keycode::new(unsafe { ffi::xkb_keymap_max_keycode(self.ptr) })
    }

    pub(crate) fn num_mods(&self) -> ModIndex {
        // SAFETY: `ptr` is a live keymap.
        unsafe { ffi::xkb_keymap_num_mods(self.ptr) }
    }

    /// The modifier's name, empty for an index the keymap does not have.
    pub(crate) fn mod_get_name(&self, index: ModIndex) -> &str {
        // SAFETY: the keymap owns the name for as long as it lives.
        unsafe { borrowed(ffi::xkb_keymap_mod_get_name(self.ptr, index)) }.unwrap_or_default()
    }

    /// The modifier's index, or `XKB_MOD_INVALID` (`u32::MAX`).
    pub(crate) fn mod_get_index(&self, name: &str) -> ModIndex {
        let Ok(name) = CString::new(name) else {
            return u32::MAX;
        };
        // SAFETY: `ptr` is a live keymap and `name` a NUL-terminated string.
        unsafe { ffi::xkb_keymap_mod_get_index(self.ptr, name.as_ptr()) }
    }

    pub(crate) fn num_layouts(&self) -> LayoutIndex {
        // SAFETY: `ptr` is a live keymap.
        unsafe { ffi::xkb_keymap_num_layouts(self.ptr) }
    }

    pub(crate) fn key_get_name(&self, key: Keycode) -> Option<&str> {
        // SAFETY: the keymap owns the name for as long as it lives.
        unsafe { borrowed(ffi::xkb_keymap_key_get_name(self.ptr, key.raw())) }
    }

    pub(crate) fn num_layouts_for_key(&self, key: Keycode) -> LayoutIndex {
        // SAFETY: `ptr` is a live keymap.
        unsafe { ffi::xkb_keymap_num_layouts_for_key(self.ptr, key.raw()) }
    }

    pub(crate) fn num_levels_for_key(&self, key: Keycode, layout: LayoutIndex) -> LevelIndex {
        // SAFETY: `ptr` is a live keymap.
        unsafe { ffi::xkb_keymap_num_levels_for_key(self.ptr, key.raw(), layout) }
    }

    pub(crate) fn key_get_syms_by_level(
        &self,
        key: Keycode,
        layout: LayoutIndex,
        level: LevelIndex,
    ) -> &[Keysym] {
        let mut syms: *const u32 = std::ptr::null();
        // SAFETY: `ptr` is a live keymap and `syms` a valid out pointer.
        let len = unsafe {
            ffi::xkb_keymap_key_get_syms_by_level(self.ptr, key.raw(), layout, level, &mut syms)
        };
        let Ok(len) = usize::try_from(len) else {
            return &[];
        };
        if syms.is_null() || len == 0 {
            return &[];
        }
        // SAFETY: the keymap owns `len` keysyms at `syms` for as long as it
        // lives, and `Keysym` is a `repr(transparent)` `u32`.
        unsafe { std::slice::from_raw_parts(syms.cast::<Keysym>(), len) }
    }

    /// The raw pointer, for libxkbcommon functions looked up at run time.
    pub(crate) fn as_ptr(&self) -> *mut ffi::xkb_keymap {
        self.ptr
    }
}

/// Keyboard state on one keymap.
pub(crate) struct State {
    ptr: *mut ffi::xkb_state,
}

impl State {
    pub(crate) fn new(keymap: &Keymap) -> State {
        // SAFETY: `keymap.ptr` is a live keymap; the state takes its own
        // reference.
        let ptr = unsafe { ffi::xkb_state_new(keymap.ptr) };
        if ptr.is_null() {
            // libxkbcommon returns NULL only when it cannot allocate.
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<usize>());
        }
        State { ptr }
    }

    pub(crate) fn update_key(&mut self, key: Keycode, direction: KeyDirection) -> StateComponent {
        // SAFETY: `ptr` is a live state.
        unsafe { ffi::xkb_state_update_key(self.ptr, key.raw(), direction as u32) }
    }

    pub(crate) fn update_mask(
        &mut self,
        depressed_mods: ModMask,
        latched_mods: ModMask,
        locked_mods: ModMask,
        depressed_layout: LayoutIndex,
        latched_layout: LayoutIndex,
        locked_layout: LayoutIndex,
    ) -> StateComponent {
        // SAFETY: `ptr` is a live state.
        unsafe {
            ffi::xkb_state_update_mask(
                self.ptr,
                depressed_mods,
                latched_mods,
                locked_mods,
                depressed_layout,
                latched_layout,
                locked_layout,
            )
        }
    }

    pub(crate) fn key_get_one_sym(&self, key: Keycode) -> Keysym {
        // SAFETY: `ptr` is a live state.
        Keysym::new(unsafe { ffi::xkb_state_key_get_one_sym(self.ptr, key.raw()) })
    }

    pub(crate) fn serialize_mods(&self, components: StateComponent) -> ModMask {
        // SAFETY: `ptr` is a live state.
        unsafe { ffi::xkb_state_serialize_mods(self.ptr, components) }
    }

    pub(crate) fn serialize_layout(&self, components: StateComponent) -> LayoutIndex {
        // SAFETY: `ptr` is a live state.
        unsafe { ffi::xkb_state_serialize_layout(self.ptr, components) }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        // SAFETY: this value holds the state's one reference.
        unsafe { ffi::xkb_state_unref(self.ptr) }
    }
}

/// The keysym's name, empty when it has none.
pub(crate) fn keysym_get_name(keysym: Keysym) -> String {
    let mut buffer = [0 as c_char; 64];
    // SAFETY: the buffer and its length match.
    let len = unsafe { ffi::xkb_keysym_get_name(keysym.raw(), buffer.as_mut_ptr(), buffer.len()) };
    if len <= 0 {
        return String::new();
    }
    // SAFETY: libxkbcommon NUL-terminates the name, truncating to the buffer.
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// The keysym named `name`, or `NoSymbol`.
pub(crate) fn keysym_from_name(name: &str, flags: KeysymFlags) -> Keysym {
    let Ok(name) = CString::new(name) else {
        return Keysym::NoSymbol;
    };
    // SAFETY: `name` is NUL-terminated.
    Keysym::new(unsafe { ffi::xkb_keysym_from_name(name.as_ptr(), flags) })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Self-contained, so the test reads no system XKB data.
    const KEYMAP: &str = r#"xkb_keymap {
        xkb_keycodes { minimum = 8; maximum = 255; <AC01> = 38; <LFSH> = 50; };
        xkb_types {
            type "ONE_LEVEL" { modifiers = none; level_name[Level1] = "Any"; };
            type "ALPHABETIC" {
                modifiers = Shift;
                map[Shift] = Level2;
                level_name[Level1] = "Base";
                level_name[Level2] = "Caps";
            };
        };
        xkb_compat { interpret Shift_L { action = SetMods(modifiers = Shift); }; };
        xkb_symbols {
            key <AC01> { type = "ALPHABETIC", [ a, A ] };
            key <LFSH> { [ Shift_L ] };
            modifier_map Shift { <LFSH> };
        };
    };"#;

    #[test]
    fn a_keymap_answers_what_typing_asks_of_it() {
        let map = Keymap::new_from_string(
            &Context::new(CONTEXT_NO_DEFAULT_INCLUDES | CONTEXT_NO_ENVIRONMENT_NAMES),
            KEYMAP.into(),
            KEYMAP_FORMAT_TEXT_V1,
            KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the test keymap compiles");
        let a = Keycode::new(38);
        let shift = Keycode::new(50);
        assert_eq!((map.min_keycode().raw(), map.max_keycode().raw()), (38, 50));
        assert_eq!(map.num_layouts(), 1);
        assert_eq!(map.num_layouts_for_key(a), 1);
        assert_eq!(map.num_levels_for_key(a, 0), 2);
        assert_eq!(map.key_get_syms_by_level(a, 0, 1), [Keysym::A]);
        assert_eq!(map.key_get_syms_by_level(Keycode::new(9), 0, 0), []);
        assert_eq!(map.key_get_name(a), Some("AC01"));
        assert_eq!(map.key_get_name(Keycode::new(9)), None);
        let shift_index = map.mod_get_index(MOD_NAME_SHIFT);
        assert_eq!(map.mod_get_name(shift_index), MOD_NAME_SHIFT);
        assert!(shift_index < map.num_mods());
        assert_eq!(map.mod_get_index("Nonexistent"), u32::MAX);
        assert_eq!(map.mod_get_index("Shi\0ft"), u32::MAX);
        assert_eq!(map.mod_get_name(u32::MAX), "");

        let mut state = State::new(&map.clone());
        assert_eq!(state.key_get_one_sym(a), Keysym::a);
        state.update_key(shift, KeyDirection::Down);
        assert_eq!(state.serialize_mods(STATE_MODS_DEPRESSED), 1 << shift_index);
        assert_eq!(state.key_get_one_sym(a), Keysym::A);
        state.update_mask(0, 0, 1 << shift_index, 0, 0, 0);
        assert_eq!(state.serialize_mods(STATE_MODS_DEPRESSED), 0);
        assert_eq!(state.serialize_mods(STATE_MODS_LOCKED), 1 << shift_index);
        assert_eq!(state.serialize_layout(STATE_LAYOUT_EFFECTIVE), 0);
    }

    #[test]
    fn a_text_that_is_not_a_keymap_or_an_unknown_format_gives_none() {
        let context = Context::new(CONTEXT_NO_DEFAULT_INCLUDES | CONTEXT_NO_ENVIRONMENT_NAMES);
        assert!(Keymap::new_from_string(
            &context,
            "not a keymap".into(),
            KEYMAP_FORMAT_TEXT_V1,
            KEYMAP_COMPILE_NO_FLAGS
        )
        .is_none());
        // libxkbcommon refuses a format it does not know.
        assert!(
            Keymap::new_from_string(&context, KEYMAP.into(), 99, KEYMAP_COMPILE_NO_FLAGS).is_none()
        );
    }

    #[test]
    fn keysyms_and_their_names_round_trip() {
        assert_eq!(keysym_get_name(Keysym::Return), "Return");
        assert_eq!(keysym_from_name("Return", KEYSYM_NO_FLAGS), Keysym::Return);
        assert_eq!(
            keysym_from_name("return", KEYSYM_CASE_INSENSITIVE),
            Keysym::Return
        );
        assert_eq!(
            keysym_from_name("return", KEYSYM_NO_FLAGS),
            Keysym::NoSymbol
        );
        assert_eq!(
            keysym_from_name("Ret\0urn", KEYSYM_NO_FLAGS),
            Keysym::NoSymbol
        );
    }
}
