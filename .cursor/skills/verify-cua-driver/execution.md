# Linux sandbox execution

Status: **blocked before driver build; not an end-to-end verification proof**.

The generated instructions were attempted from the repository root:

```bash
export PATH="$HOME/.cargo/bin:$PATH" CARGO_NET_OFFLINE=true
timeout 1260 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11
```

The helper launched Xvfb, a private D-Bus session, openbox and picom. The WM probe passed and D-Bus activated `org.a11y.Bus`; its address was captured. The canonical runner then failed with exit 101 because the offline Cargo registry lacks `cap-fs-ext`. An earlier focused Cargo test attempted online resolution and failed because `index.crates.io` could not resolve. There was no source-built binary or staged fixture available to reuse.

The wrapper performed cleanup and confirmed evidence remained at:

```text
artifacts/cua-driver/verify/run-1LZFNB3J/
  launch.log
  openbox.log
  picom.log
  at-spi-address.txt
  cases.jsonl
  environment.jsonl
  results.jsonl
```

The three JSONL result files are empty: no daemon readiness, doctor, click delivery, screenshot, sentinel observation or video was proved. Desktop setup logs are not substitutes for those artifacts. Dconf also warned that the sandbox's home cache was read-only. Retry the documented invocation with dependencies available; do not call this attempt a passing verification.

The two new focused Rust tests could not execute because dependency resolution fails before compilation. Rust formatting, shell syntax and whitespace checks are separate from behavioral verification. Prettier invocation was also unavailable: the repository's pnpm shim needed an uncached package-manager download, and offline npm had no Prettier package.

Sandbox-only setup used the installed Rust toolchain on PATH and offline Cargo mode to fail promptly after the observed DNS failure. No fixture result was forged, no external prebuilt driver was substituted and no test-only setter was used. A successful future X11 run still uses the stand-ins listed in SKILL.md: virtual desktop, fixture app, disposable state and authorization bypass. macOS, Windows and Wayland remain unverified.
