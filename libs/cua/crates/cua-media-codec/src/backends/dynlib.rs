// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Dynamic library loading behind a trait, so hardware probes can be unit
//! tested with fake loaders on any host.

use std::collections::HashMap;
use std::ffi::c_void;

/// A loaded shared library.
pub trait Library: Send + Sync {
    /// Resolves a symbol, `None` if absent.
    fn symbol(&self, name: &str) -> Option<*const c_void>;
    /// Path or name it was loaded from.
    fn name(&self) -> &str;
}

/// Opens shared libraries.
pub trait LibraryLoader: Send + Sync {
    /// Opens the first library in `candidates` that loads.
    fn open(&self, candidates: &[&str]) -> Result<Box<dyn Library>, String>;
    /// Opens a DRM render node (Linux) and returns its fd. The default
    /// implementation uses the real filesystem.
    fn open_render_node(&self, path: &str) -> Option<i32> {
        #[cfg(unix)]
        {
            use std::os::fd::IntoRawFd;
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .ok()
                .map(IntoRawFd::into_raw_fd)
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            None
        }
    }
    /// Closes an fd returned by [`Self::open_render_node`].
    fn close_fd(&self, fd: i32) {
        #[cfg(unix)]
        {
            use std::os::fd::FromRawFd;
            drop(unsafe { std::fs::File::from_raw_fd(fd) });
        }
        #[cfg(not(unix))]
        let _ = fd;
    }
}

/// The real loader (libloading).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemLoader;

struct SystemLibrary {
    lib: libloading::Library,
    name: String,
}

impl Library for SystemLibrary {
    fn symbol(&self, name: &str) -> Option<*const c_void> {
        let mut cname = name.as_bytes().to_vec();
        cname.push(0);
        // SAFETY: we only read the symbol address; callers transmute it to
        // the documented signature.
        unsafe {
            self.lib
                .get::<*const c_void>(&cname)
                .ok()
                .map(|s| *s)
                .filter(|p| !p.is_null())
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl LibraryLoader for SystemLoader {
    fn open(&self, candidates: &[&str]) -> Result<Box<dyn Library>, String> {
        let mut errors = Vec::new();
        for name in candidates {
            // SAFETY: loading a vendor runtime runs its initialisers; that is
            // the documented way to use these SDKs. Probes run in an isolated
            // child process when the daemon asks for isolation.
            match unsafe { libloading::Library::new(name) } {
                Ok(lib) => {
                    return Ok(Box::new(SystemLibrary {
                        lib,
                        name: (*name).to_owned(),
                    }))
                }
                Err(e) => errors.push(format!("{name}: {e}")),
            }
        }
        Err(errors.join("; "))
    }
}

/// A fake library for tests: a name → function pointer table.
#[derive(Default)]
pub struct FakeLibrary {
    /// Name reported by [`Library::name`].
    pub name: String,
    /// Exported symbols.
    pub symbols: HashMap<String, usize>,
}

impl Library for FakeLibrary {
    fn symbol(&self, name: &str) -> Option<*const c_void> {
        self.symbols.get(name).map(|p| *p as *const c_void)
    }
    fn name(&self) -> &str {
        &self.name
    }
}

/// A fake loader for tests. Libraries are produced by a factory keyed by
/// candidate name; render nodes are simulated by a list of paths.
#[derive(Default)]
pub struct FakeLoader {
    /// Library name → symbol table.
    pub libraries: HashMap<String, HashMap<String, usize>>,
    /// Render nodes that "exist" (their index is used as the fd).
    pub render_nodes: Vec<String>,
}

impl FakeLoader {
    /// Adds a library exposing `symbols`.
    pub fn with_library(mut self, name: &str, symbols: &[(&str, usize)]) -> Self {
        self.libraries.insert(
            name.to_owned(),
            symbols.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
        );
        self
    }
}

impl LibraryLoader for FakeLoader {
    fn open(&self, candidates: &[&str]) -> Result<Box<dyn Library>, String> {
        for name in candidates {
            if let Some(symbols) = self.libraries.get(*name) {
                return Ok(Box::new(FakeLibrary {
                    name: (*name).to_owned(),
                    symbols: symbols.clone(),
                }));
            }
        }
        Err(format!("fake: none of {candidates:?} present"))
    }

    fn open_render_node(&self, path: &str) -> Option<i32> {
        self.render_nodes
            .iter()
            .position(|p| p == path)
            .map(|i| 1000 + i as i32)
    }

    fn close_fd(&self, _fd: i32) {}
}

/// Resolves `name` from `lib` and transmutes it to the function type `F`.
///
/// # Safety
/// `F` must be the symbol's real C signature.
pub unsafe fn resolve<F: Copy>(lib: &dyn Library, name: &str) -> Result<F, String> {
    assert_eq!(
        std::mem::size_of::<F>(),
        std::mem::size_of::<*const c_void>()
    );
    let ptr = lib
        .symbol(name)
        .ok_or_else(|| format!("{} does not export {name}", lib.name()))?;
    Ok(unsafe { std::mem::transmute_copy::<*const c_void, F>(&ptr) })
}
