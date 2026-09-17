# Visual perception demo evidence

This directory defines the review-only evidence boundary for the visual canvas
demo. The current GitHub workflow runs mock/static checks and the canonical
Windows and Linux X11 Driver harnesses. It has no protected environment, API
key, live Jev call, or artifact upload. Live jobs remain disabled until a
separately reviewed callable adapter is committed.

`sanitize_evidence.py` measures the checked-out source SHA and host platform,
reads the fixture's loopback oracle and adapter result, hashes the model,
extension, and recording bytes, and verifies the extension's detached
RSA-SHA256 signature against a public key whose digest is independently
approved. Its output is limited to `manifest.json` and `recording.mp4`.

macOS is intentionally separate from this Windows/Linux workflow. Native macOS
proof must use the logged-in, TCC-authorized Lume runner and the canonical
`libs/cua-driver/tests/runners/macos-lume/run-all.sh --standalone-browser`
harness before a macOS demo lane is added.
