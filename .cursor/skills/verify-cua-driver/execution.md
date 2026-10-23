# Linux execution outcome

The documented `timeout 1260 .cursor/skills/verify-cua-driver/scripts/verify-linux.sh x11` invocation was retried. X11 session setup and the release driver build succeeded, but Electron dependency staging failed. A fresh-lockfile installation retry also failed; the npm registry was unreachable and no cached Electron runtime was available.

Setup logs survived cleanup. Doctor, preflight and both click cells did not run: there are no delivered summary rows, daemon diagnostics, transcripts, fixture-state snapshots, sentinel/native observations, screenshots or videos. No surviving-owned-PID warning was printed, but there were no recorded daemon/fixture PIDs to check. The transcript PID extraction, live-process cleanup check and fixture snapshot-on-drop paths remain unverified on real output. This is not an end-to-end proof; retry on a host with registry access or a complete fixture dependency/runtime cache.

## Stand-ins

- Xvfb instead of a physical desktop session.
- The real Electron fixture instead of a user's application; it did not launch in these attempts.
- Electron sandbox/GPU disabled, forced accessibility and a fixture CDP port.
- Disposable driver HOME/XDG state, disabled overlay/permissions gate and testkit unrestricted GUI authorization/approval bypass.
- Fixture control only establishes sentinel posture; it does not set the target action result.

These do not certify production admission, interactive consent, physical GPU capture or arbitrary app compatibility. Wayland, macOS and Windows remain unverified.
