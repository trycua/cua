#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| cua_driver_testkit::boundary_fuzz::tool_arguments(data));
