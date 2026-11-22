// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The tool registry keeps the process MTA alive (#2298).
//!
//! windows-rs caches agile WinRT activation factories, such as the
//! `Direct3D11CaptureFramePool` statics used for WGC capture, in process
//! statics. If every thread that joined the MTA leaves it (a paired
//! `CoUninitialize`, or a tokio blocking thread exiting), COM tears the MTA
//! down and unloads the factory's DLL, and the next call through the cache
//! faults reading a vtable. This lives in its own test binary so no other
//! test's apartment can hold the MTA open.

#![cfg(target_os = "windows")]

use windows::Win32::System::Com::{
    CoGetApartmentType, CoInitializeEx, CoUninitialize, APTTYPE, APTTYPEQUALIFIER,
    APTTYPEQUALIFIER_IMPLICIT_MTA, APTTYPE_MTA, COINIT_MULTITHREADED,
};

/// Apartment seen by a fresh thread that never initialized COM.
fn fresh_thread_apartment() -> windows::core::Result<(APTTYPE, APTTYPEQUALIFIER)> {
    std::thread::spawn(|| {
        let mut kind = APTTYPE::default();
        let mut qualifier = APTTYPEQUALIFIER::default();
        unsafe { CoGetApartmentType(&mut kind, &mut qualifier) }.map(|()| (kind, qualifier))
    })
    .join()
    .unwrap()
}

/// Join the MTA and leave it again on a thread that then exits.
fn enter_and_leave_mta() {
    std::thread::spawn(|| unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        CoUninitialize();
    })
    .join()
    .unwrap();
}

#[test]
fn registry_keeps_the_mta_alive_after_every_member_thread_leaves() {
    enter_and_leave_mta();
    assert!(
        fresh_thread_apartment().is_err(),
        "no MTA should exist before the registry is built"
    );

    let _registry = platform_windows::tools::build_registry(false);
    enter_and_leave_mta();

    let (kind, qualifier) = fresh_thread_apartment().expect("the MTA outlives its member threads");
    assert_eq!(kind, APTTYPE_MTA);
    assert_eq!(qualifier, APTTYPEQUALIFIER_IMPLICIT_MTA);
}
