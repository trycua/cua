:robot: I have created a release *beep* *boop*
---


## What changes for users

- Cua Spaces on a Mac that cannot run Spaces itself (for example a virtual machine) now says so in plain words and points you to another computer or your own cloud.
- Space creates that fail now clean up the virtual machines and containers they leave behind, and say what is using a full disk.
- Local macOS Spaces reach their virtual machine reliably, even with VPN or Local Network restrictions.
- A downloaded update now installs when you quit the app.
- Windows Spaces on cloud hosts, the new first-run setup (Settings, Advanced, Labs, "Try the new setup") and Secure Spaces, child Spaces and the macOS developer-tools image are experiments. They are off by default and only appear when turned on.

Smoke test: update from 0.19.4 to the 0.20.0 build installed on quit, the app launched, Settings and Labs render with the new setup switch off, sign-up and sign-in worked, and the CLI reported its version and listed Spaces.

## [0.20.0](https://github.com/trycua/cua/compare/cua-spaces-v0.19.4...cua-spaces-v0.20.0) (2026-11-03)


### Features

* **spaces:** Secure Spaces, child Spaces, conformance, space jobs and the macOS developer-tools image, behind experiments ([#6306](https://github.com/trycua/cua/issues/6306)) ([4715a72](https://github.com/trycua/cua/commit/4715a723a450610cdf77f688776c1fdd40660149))

---
This PR was generated with [Release Please](https://github.com/googleapis/release-please). See [documentation](https://github.com/googleapis/release-please#release-please).
