# Transient Seat Container Test Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a required, container-based NixOS integration test for Cua Driver transient Wayland seats with inspectable GIF and log evidence.

**Architecture:** A NixOS `containers.machine` test provisions the headless Cua compositor and Rust test runtime, then invokes the existing ignored Rust transient-seat contract through the compositor injection socket. The Nix harness collects a GIF and logs before returning the Rust command's status; GitHub Actions uploads the produced evidence on both success and failure.

**Tech Stack:** Nix flakes, NixOS test driver/systemd-nspawn, wlroots Cua compositor, Rust/Cargo E2E test, GitHub Actions artifacts.

## Global Constraints

- Keep transient-seat behavioral assertions exclusively in `transient_seat_behavior_test.rs`.
- Use `containers.machine`; do not add QEMU or VM-only device dependencies.
- Require the check in normal pull-request CI.
- Export nonempty GIF and logs before a failed test aborts the Nix test driver.

---

### Task 1: Add the failing container integration check

**Files:**
- Create: `nix/cua-driver/tests/transient-seat.nix`
- Modify: `flake.nix`
- Test: `nix build .#checks.x86_64-linux.cua-driver-transient-seat`

**Interfaces:**
- Consumes: `cuaDriver`, `cuaCompositor`, and the Rust workspace source supplied by `flake.nix`.
- Produces: `checks.cua-driver-transient-seat`, with GIF and log files in `$out`.

- [ ] Define a `containers.machine` NixOS test that installs the compositor, Rust build/runtime inputs, and GIF capture tools.
- [ ] Have `testScript` start a private D-Bus session, the headless compositor, and wait for both the Wayland and injection sockets.
- [ ] Run `cargo test -p cua-driver --test transient_seat_behavior_test -- --ignored --nocapture --test-threads=1`, recording its exit status.
- [ ] Stop the recorder, assert that the GIF is nonempty, copy evidence from the container, and only then fail for a nonzero Rust test status.
- [ ] Register the new flake check and run the focused build; it must initially fail until CI wiring is added.

### Task 2: Make evidence available in required PR CI

**Files:**
- Modify: `.github/workflows/ci-nix-linux.yml`
- Test: pull-request workflow check selection and artifact upload paths

**Interfaces:**
- Consumes: the Nix derivation output from `cua-driver-transient-seat`.
- Produces: a required check and retained GIF/log artifact for every run.

- [ ] Add the transient-seat flake check to the Nix PR test matrix with the container runtime's `auto-allocate-uids`, `cgroups`, and `uid-range` features.
- [ ] Upload its output directory with `if: always()` so failed runs retain GIF and logs.
- [ ] Verify the workflow's check name matches the flake attribute exactly.

### Task 3: Validate focused and structural contracts

**Files:**
- Modify: `nix/cua-driver/tests/README.md`
- Test: Nix evaluation/build and workflow YAML parsing

**Interfaces:**
- Consumes: the check and workflow from Tasks 1-2.
- Produces: documented container-test ownership boundaries.

- [ ] Document that this check provisions a headless container session and delegates behavioral assertions to the typed Rust test.
- [ ] Run the focused Nix check and inspect the output GIF/log files when Nix is available.
- [ ] Run repository-available formatting or YAML validation without changing unrelated files.
