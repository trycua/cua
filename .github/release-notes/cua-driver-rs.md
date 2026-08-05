## Install

macOS and Linux:

```bash
/bin/bash -c "$(curl -fsSL https://cua.ai/driver/install.sh)"
```

Windows:

```powershell
irm https://cua.ai/driver/install.ps1 | iex
```

Cua Driver ships universal macOS binaries, x86_64 and arm64 Windows builds,
and Linux preview builds. Release assets also include tag-pinned installer and
uninstaller scripts.

## Why GitHub says “Pre-release”

GitHub's label is used only to keep this monorepo's repository-wide “Latest”
pointer from switching between independently released products. A plain Cua
Driver SemVer such as `0.17.0` is a stable release; npm and PyPI publish it on
their normal stable channels, and the installers resolve the versioned
`cua-driver-rs-v*` releases directly.
