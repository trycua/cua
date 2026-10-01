# Cua installers

Two scripts install the `cua` CLI, the Cua Spaces app and agent extras, then sign in and set up your AI coding agents with `cua auth login`.

```sh
curl -fsSL https://cua.ai/install.sh | sh
```

```powershell
irm https://cua.ai/install.ps1 | iex
```

The Cua Spaces app is the graphical path. Its first run installs the bundled `cua` CLI onto PATH, signs in and runs the same agent onboarding (`cua agents setup`).

## Choose what to install

In a terminal, install.sh shows a checklist (arrow or number keys to move and toggle, space to toggle, Enter to install; install.ps1 asks for numbers to toggle). The CLI row is locked. Without a terminal, with `--yes`, or with `CUA_INSTALL_NONINTERACTIVE=1`, the defaults plus `--select` are installed without asking. `--only` installs exactly the listed items plus the CLI and never shows the checklist.

| Item | What it does | Default |
|---|---|---|
| `cli` | The `cua` CLI | Always (not with `--app-only`) |
| `spaces` | The Cua Spaces app | On for macOS. macOS only for now: on Linux and Windows it is hidden, and selecting it (`--select`/`--only spaces`, `--app-only`, `--mode`) skips the app with a note. `--app-only` and `--mode` imply it |
| `cua-driver` | Installs cua-driver with its own release installer (`cua.ai/driver/install.sh` / `install.ps1`, which verify SHA256SUMS and the Sigstore bundle), then `cua agents setup --cua-driver --agents all --yes` registers its skill and MCP server for your agents | Off |
| `host` | Runs `cua host setup` after sign-in so others on your account can reach this machine | Off |

```sh
curl -fsSL https://cua.ai/install.sh | sh -s -- --select cua-driver   # checklist with cua-driver ticked
curl -fsSL https://cua.ai/install.sh | sh -s -- --only cua-driver,host # no checklist
```

For cua-driver, `--prefix DIR` becomes `--bin-dir DIR/bin`, `--require-signature` is passed on, and `--no-modify-path` (`-NoPathUpdate`) is passed unless you give `--modify-path`. The driver installer needs bash.

## Options

| install.sh | install.ps1 | Effect |
|---|---|---|
| `--select a,b` | `-Select a,b` | Preselect items (`spaces`, `cua-driver`, `host`); the checklist still shows |
| `--only a,b` | `-Only a,b` | Exactly these items plus the CLI, no checklist |
| `--cli-only` | `-CliOnly` | Only the `cua` CLI (same as `--only cli`) |
| `--app-only` | `-AppOnly` | Only the Cua Spaces app, no CLI (cannot be combined with `cua-driver` or `host`) |
| `--mode host\|client` | `-Mode host\|client` | Preselect the app's first-run choice (implies `spaces`) |
| `--version 1.2.3` | `-Version 1.2.3` | Pin the CLI release (default: latest) |
| `--prefix DIR` | `-Prefix DIR` | Install under `DIR/bin` (and `DIR/Applications` on macOS) |
| `--modify-path` | `-ModifyPath` | Add the bin dir to your shell profile or user PATH |
| `--no-onboarding` | `-NoOnboarding` | Skip `cua auth login` |
| `--require-signature` | `-RequireSignature` | Fail unless minisign or cosign verifies |
| `--dry-run` | `-DryRun` | Print the plan, change nothing |
| `--yes` | `-Yes` | Accept the current selection and every prompt |
| | `-Installer msi\|nsis` | Windows app installer (default msi) |

Pass install.sh options through the pipe with `sh -s --`, for example `curl -fsSL https://cua.ai/install.sh | sh -s -- --cli-only --yes`. `--only` and `--select` cannot be combined, nor can `--cli-only` with other items. Set `CUA_INSTALL_NONINTERACTIVE=1` to never prompt.

## Where things go

| | CLI | App |
|---|---|---|
| macOS | `~/.local/bin/cua` (`/usr/local/bin` as root) | `/Applications` if writable, else `~/Applications` (from the .dmg; the SwiftUI app, macOS 26 or later, skipped on older macOS) |
| Linux | same as macOS | Not offered for now (macOS only). When re-enabled: AppImage at `~/.local/bin/cua-spaces` plus a desktop entry; the .deb via apt-get when run as root |
| Windows | `%LOCALAPPDATA%\Programs\cua\bin\cua.exe` | Not offered for now (macOS only). When re-enabled: silent MSI (`CUA_SPACES_MODE=`) or NSIS (`/S /MODE=`) |

cua-driver lands where its own installer puts it: `~/.local/bin/cua-driver` (or `DIR/bin` with `--prefix`), `%LOCALAPPDATA%\Programs\Cua\cua-driver\bin` on Windows.

The scripts never edit shell profiles unless you pass `--modify-path`. `--mode` writes `~/.cua/spaces-install-mode`, which the app reads on first launch.

MDM on macOS: `apps/cua-spaces/scripts/build-pkg.sh` builds a `.pkg` (signed with `--sign`, install mode baked with `--mode`) that installs the app and links `/usr/local/bin/cua` to the CLI bundled inside it. The macOS app is the SwiftUI app (`apps/cua-spaces-macos`, `scripts/build-release.sh`), which carries the CLI as `Contents/MacOS/cua`; the Linux and Windows (Tauri) bundles, not released for now, carry it as a sidecar (`apps/cua-spaces/scripts/prepare-cli-sidecar.sh` + `src-tauri/tauri.sidecar.conf.json`).

## Release manifest

Both scripts read `release-artifacts.json`:

- latest: `https://github.com/trycua/cua/releases/download/cua-install-latest/release-artifacts.json` (a rolling release the CD workflows update)
- pinned: `https://github.com/trycua/cua/releases/download/cua-sdk-v<version>/release-artifacts.json`

The copies of `install.sh` / `install.ps1` on a repository's `cua-install-latest` release default to that repository: the release workflows publish them through `stamp_installers.py --repo "$GITHUB_REPOSITORY"`, so the trycua/cua-staging feed downloads, and verifies Sigstore identities against, trycua/cua-staging's own releases. The files in this directory keep `trycua/cua`.

`release_manifest.py` writes it from a directory of release files. Names it recognises:

| Component | File |
|---|---|
| CLI | `cua-cli-<ver>-<platform>.tar.gz` (macOS, Linux), `cua-cli-<ver>-windows-<arch>.zip` |
| App | `cua-spaces-<ver>-darwin-universal.dmg` / `.pkg`, `cua-spaces-<ver>-linux-<arch>.AppImage` / `.deb`, `cua-spaces-<ver>-windows-<arch>.msi` / `-setup.exe` |

`<platform>` is `darwin-arm64`, `darwin-x64`, `linux-x64`, `linux-arm64`, `windows-x64` or `windows-arm64`. Each entry carries `url`, `sha256` and, when present next to the file, `minisig` and `cosign_bundle` URLs. Every artifact sits on one line so install.sh can read it without jq.

Downloads always check sha256. Signatures are checked when the entry lists one and `minisign` (with `CUA_INSTALL_MINISIGN_PUBKEY` or the key baked into the script) or `cosign` is installed.

## Tests

```sh
scripts/install/tests/run.sh                     # install.sh against a local fake release
python3 -m unittest discover scripts/install/tests
scripts/install/tests/docker-pester.sh           # install.ps1 Pester tests in a container
```

`run.sh` uses a fake HOME and `--prefix` temp dirs; it never writes to your real home, `/Applications` or PATH. On macOS it also builds and mounts a throwaway .dmg. The `installer-smoke` job in `.github/workflows/ci-installers.yml` runs all of these on Linux, macOS and Windows.

Test hooks: `CUA_INSTALL_MANIFEST_URL`, `CUA_INSTALL_BASE_URL`, `CUA_INSTALL_DRIVER_URL` (the cua-driver installer, same HTTPS rules), `CUA_INSTALL_OS`, `CUA_INSTALL_ARCH`. With util-linux `script`, run.sh drives the checklist through a pty.
