//! Desktop E2E suites for `cua-driver`.
//!
//! This crate has no library code. Its integration tests in `tests/` drive a
//! built `cua-driver` binary against real desktops and staged harness apps.
//! Most suites are `#[ignore]`d and run only through the canonical OS runners
//! listed in `libs/cua-driver/docs/test-harnesses-guide.md`.
