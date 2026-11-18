// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay `meta` keys the phone's lists read are the ones the hosts and the
//! clouds write: this fails when either side renames one.

use cua_spaces_app_core::phone::directory::meta;

#[test]
fn the_pool_keys_are_the_hosts_and_the_clouds() {
    assert_eq!(meta::PROVIDES_SPACES, cua_host::META_PROVIDES_SPACES);
    assert_eq!(meta::OS, cua_host::META_OS);
    assert_eq!(meta::ARCH, cua_host::META_ARCH);
    assert_eq!(meta::CLOUD_HOST, cua_host::META_CLOUD);
    assert_eq!(meta::PHONE_FEED, cua_host::META_PHONE_FEED);
    assert_eq!(meta::CLOUD_PROVIDER, cua_sandbox_core::byoc::meta::PROVIDER);
    assert_eq!(meta::CLOUD_PLACE, cua_sandbox_core::byoc::meta::PLACE);
}
