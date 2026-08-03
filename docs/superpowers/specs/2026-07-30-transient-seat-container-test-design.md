# Transient Seat Container Test Design

## Goal

Make transient-seat behavior a required pull-request check using the NixOS
container test runtime and retain visual and textual evidence from every run.

## Scope

- Run the existing ignored Rust transient-seat behavior test in a headless
  NixOS container with the Cua compositor and its private injection socket.
- Keep behavioral assertions in Rust; the Nix test owns environment setup,
  evidence collection, and artifact export only.
- Record a GIF, compositor log, and Rust test log before propagating the test
  result, so failed runs remain inspectable.
- Register the check in `flake.nix` and the normal pull-request Nix workflow.

## Constraints

- Use `containers.machine`, not QEMU `nodes.machine`.
- Do not require `/dev/uinput`, DRM, or a logind seat; the compositor runs
  headlessly through its control socket.
- Copy artifacts from the test container into the derivation output, then make
  CI upload that output even when the check fails.
