// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| cua_driver_testkit::boundary_fuzz::tool_arguments(data));
