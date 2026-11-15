// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Typed entry points for libraries whose availability is checked at runtime.

macro_rules! entry_points {
    ($table:ident, $sys:ident, { $($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)* }) => {
        pub(crate) struct $table {
            $(pub(crate) $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            _library: libloading::Library,
        }

        impl $table {
            /// # Safety
            /// The caller must satisfy the library's load-time initialisers.
            unsafe fn open(soname: &str) -> Result<Self, libloading::Error> {
                // SAFETY: the caller establishes the library's load-time requirements.
                let library = unsafe { libloading::Library::new(soname) }?;
                $(
                    // Type-check against the binding without linking the symbol.
                    const _: unsafe extern "C" fn($($arg),*) $(-> $ret)? = $sys::$name;
                    // SAFETY: the symbol has the type checked above; the table
                    // owns the library for as long as its pointers are used.
                    let $name = *unsafe {
                        library.get::<unsafe extern "C" fn($($arg),*) $(-> $ret)?>(
                            concat!(stringify!($name), "\0").as_bytes(),
                        )
                    }?;
                )*
                Ok(Self { $($name,)* _library: library })
            }
        }
    };
}

pub(crate) use entry_points;
