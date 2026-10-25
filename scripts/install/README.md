# Cua installers

Two scripts install the `cua` CLI, the Cua Spaces app and agent extras, then sign in and set up your AI coding agents with `cua auth login`. Two more remove them again: [`uninstall.sh` and `uninstall.ps1`](#uninstall).

```sh
curl -fsSL https://cua.ai/install.sh | sh
```

```powershell
irm https://cua.ai/install.ps1 | iex
```

The Cua Spaces app is the graphical path. Its first run installs the bundled `cua` CLI onto PATH, signs in and runs the same agent onboarding (`cua agents setup`).

## Choose what to install

In a terminal, install.sh first says what Cua Spaces is, then lists what to add to the command-line tools (the `cua` CLI; arrow or number keys to move or toggle, space to select, Enter to install; install.ps1 asks for numbers to toggle, with a line under each row). The command-line tools are always installed, so they are not a row. Letting your other devices use this computer (`cua host setup`) is not a row either: the app's first run asks (`--mode host|client` preselects the answer). Without a terminal, with `--yes`, or with `CUA_INSTALL_NONINTERACTIVE=1`, the defaults plus `--select` are installed without asking. `--only` installs exactly the listed items plus the CLI and never shows the list.

| Item | What it does | In the list | Default |
|---|---|---|---|
| `cli` | The `cua` CLI | No, always installed (not with `--app-only`) | Always |
| `spaces` | The Cua Spaces app | "Cua Spaces app: lets your AI agents use computers" | On for macOS (the SwiftUI app), Linux (the Electron app) and Windows (the Electron app). `--app-only` and `--mode` imply it |
| `cua-driver` | Installs Cua Driver with its own release installer (`cua.ai/driver/install.sh` / `install.ps1`, which verify SHA256SUMS and the Sigstore bundle), then `cua agents setup --cua-driver --agents all --yes` registers its skill and MCP server for your agents | "Cua Driver: lets AI agents use your apps while you keep working" | Off |
| `host` | Runs `cua host setup` after sign-in, so your other devices can use this computer through the relay | No: a script flag (`--select host`, `--only host`, `--pair`). Interactively, the app's first run asks | Off |

The Cua Spaces app registers the cua-driver skill and MCP server with your agents in its first run, but it does not install or update the Cua Driver program yet (CUA-1440). Until it does, the `cua-driver` item is how the installer puts Cua Driver on this computer.

```sh
curl -fsSL https://cua.ai/install.sh | sh -s -- --select cua-driver   # checklist with cua-driver ticked
curl -fsSL https://cua.ai/install.sh | sh -s -- --only cua-driver,host # no checklist
```

For cua-driver, `--prefix DIR` becomes `--bin-dir DIR/bin`, `--require-signature` is passed on, and `--no-modify-path` (`-NoPathUpdate`) is passed unless you give `--modify-path`. The driver installer needs bash.

## Options

| install.sh | install.ps1 | Effect |
|---|---|---|
| `--select a,b` | `-Select a,b` | Preselect items (`spaces`, `cua-driver`, `host`); the list still shows |
| `--only a,b` | `-Only a,b` | Exactly these items plus the CLI, no checklist |
| `--cli-only` | `-CliOnly` | Only the `cua` CLI (same as `--only cli`) |
| `--app-only` | `-AppOnly` | Only the Cua Spaces app, no CLI (cannot be combined with `cua-driver` or `host`) |
| `--mode host\|client` | `-Mode host\|client` | Preselect the app's first-run choice (implies `spaces`) |
| `--version 1.2.3` | `-Version 1.2.3` | Pin the CLI release (default: latest) |
| `--prefix DIR` | `-Prefix DIR` | Install under `DIR/bin` (and `DIR/Applications` on macOS) |
| `--modify-path` | `-ModifyPath` | Add the bin dir to your shell profile or user PATH without asking |
| `--no-modify-path` | `-NoModifyPath` | Never edit the profile or user PATH; print the command instead |
| `--no-onboarding` | `-NoOnboarding` | Skip `cua auth login` |
| `-v`, `--verbose` | `-Verbose` | Print every step: platform, manifest, each download, signature checks. Also `CUA_INSTALL_VERBOSE=1` |
| `--require-signature` | `-RequireSignature` | Fail unless minisign or cosign verifies |
| `--dry-run` | `-DryRun` | Print the plan, change nothing |
| `--yes` | `-Yes` | Accept the current selection and every prompt |
| | `-Installer msi\|nsis` | Windows app installer (default nsis, the Electron app; msi only when a release has one) |

Pass install.sh options through the pipe with `sh -s --`, for example `curl -fsSL https://cua.ai/install.sh | sh -s -- --cli-only --yes`. `--only` and `--select` cannot be combined, nor can `--cli-only` with other items. Set `CUA_INSTALL_NONINTERACTIVE=1` to never prompt.

## Partial releases

A Cua Spaces patch can ship one platform only (a macOS fix does not rebuild Windows or Linux). The release manifest (`release-artifacts.json`, also the rolling `cua-install-latest` copy) then holds a different version per platform: the release builds its own platforms and `release_manifest.py --carry-forward-platforms` keeps the earlier entries for the others, each with its own `version` and download URL. `install.sh` and `install.ps1` read only the entries for their own platform, so each installs the newest build that exists for it. `tests/test_mixed_versions.py` and the `mixed versions` Pester tests cover this.

## Telemetry

Both installers send three anonymous events about their own run (`cua_installer_step`: `installer_started`, then `installer_completed` or `installer_failed` with the stage and a fixed reason word), and print a two-line notice before the first one. They follow the switches every Cua program uses: `DO_NOT_TRACK=1`, `CUA_TELEMETRY=0`, `[telemetry] enabled = "off"` in `$CUA_HOME/config.toml`, and off in CI unless `CUA_TELEMETRY=1`. `--dry-run` sends nothing. They create or reuse `$CUA_HOME/telemetry/install_id`, the anonymous id the CLI and app send with, and never send a name, path, URL or error message. The tests set `CUA_TELEMETRY_FORBID_NETWORK=1`, which allows only a loopback `CUA_TELEMETRY_ENDPOINT`. See [Telemetry and privacy](https://cua.ai/docs/cua-sdk/concepts/telemetry).

## What it prints

One plain line per step ("Installing the command-line tools to ...", "Installing the Cua Spaces app to ..."), then what was installed and what to do next: open Cua Spaces, or sign in and run `cua agents setup` to connect your AI agents. `--verbose` adds `cua-install:` lines with the platform, the manifest, each download and the signature checks.

Every download is checked against its sha256 from the release manifest, and the summary says "(downloads checked)". Signatures are checked as well when `minisign` or `cosign` is installed; without one, a note shows only with `--verbose`, and `--require-signature` turns it into an error.

Questions read from the terminal, never from the script on stdin, so they work under `curl | sh`. With no terminal to ask on, the usual choices apply and the installer says so once. End of input (Ctrl-D, a closed terminal) is never an answer: the install questions take their default, and the PATH question leaves your profile alone.

## Where things go

| | CLI | App |
|---|---|---|
| macOS | `~/.local/bin/cua` (`/usr/local/bin` as root) | `/Applications` if writable, else `~/Applications` (from the .dmg; the SwiftUI app, macOS 26 or later, skipped on older macOS) |
| Linux | same as macOS | The Electron app: on Debian/Ubuntu (x64, arm64) the .deb via apt-get when run as root or with passwordless sudo; otherwise the AppImage at `~/.local/bin/cua-spaces` plus a desktop entry |
| Windows | `%LOCALAPPDATA%\Programs\cua\bin\cua.exe` | The Electron app's NSIS `setup.exe` for the machine's architecture (sha256 and cosign checked, plus a mandatory Authenticode signature check for signer Cua AI, Inc.), run silently (`/S /MODE=`); a silent MSI (`CUA_SPACES_MODE=`) only if a release has one |

cua-driver lands where its own installer puts it: `~/.local/bin/cua-driver` (or `DIR/bin` with `--prefix`), `%LOCALAPPDATA%\Programs\Cua\cua-driver\bin` on Windows.

On Linux and macOS, when the bin dir is not on PATH, install.sh in a terminal asks once ("Add ... to PATH in ~/.zshrc? [Y/n]", Enter = yes). Without a terminal, or with `--yes`, it prints the exact one-line command for your shell instead and never edits a profile unless you pass `--modify-path`; `--no-modify-path` never edits it. `--pair` runs the CLI by its absolute path, so it works before PATH changes. install.ps1 adds the user PATH by default (`-NoModifyPath` opts out). `--mode` writes `~/.cua/spaces-install-mode`, which the app reads on first launch.

`--pair` (`-Pair`) on a computer that already has Cua reuses what is there: a `cua` that knows `host setup --pair` (on PATH, in `~/.local/bin`, or inside the Cua Spaces app) and, on macOS and Linux, the installed app. It then downloads nothing and does not read the release manifest. `--prefix`, `--version` and `CUA_INSTALL_REINSTALL=1` install anyway; an older `cua` without `--pair` is replaced as before.

MDM on macOS: `apps/cua-spaces/scripts/build-pkg.sh` builds a `.pkg` (signed with `--sign`, install mode baked with `--mode`) that installs the app and links `/usr/local/bin/cua` to the CLI bundled inside it. The macOS app is the SwiftUI app (`apps/cua-spaces-macos`, `scripts/build-release.sh`), which carries the CLI as `Contents/MacOS/cua`; the Linux and Windows Electron app (`apps/cua-spaces-desktop`) bundles it itself, and the Tauri bundles (disabled) carry it as a sidecar (`apps/cua-spaces/scripts/prepare-cli-sidecar.sh` + `src-tauri/tauri.sidecar.conf.json`).

## Uninstall

```sh
curl -fsSL https://cua.ai/uninstall.sh | sh
```

```powershell
irm https://cua.ai/uninstall.ps1 | iex
```

It lists what it found, asks once (`--yes` skips the question; without a terminal it refuses to remove anything unless you pass `--yes`), and can be run again at any time: a computer with nothing left, or only part of Cua, is fine. It only touches Cua's own paths and never a file outside them.

| install.sh | install.ps1 | Effect |
|---|---|---|
| (default) | (default) | Removes the app, the CLI, the host service, Cua Spaces' agent skills and MCP entry, launch at login, your sign-in and Cua's settings and caches. Keeps Spaces and their data, and Cua Driver's and other tools' entries in your agents |
| `--keep-data` | `-KeepData` | Keeps `~/.cua` (settings, the Spaces registry, the Cua Volume cache, the device key) and the app's settings. Still removes programs, services, integrations and the sign-in |
| `--delete-spaces` | `-DeleteSpaces` | Also deletes the Spaces on this computer and their data: Space disks, the local Cua Volume files, the Keyvault. Running Spaces are listed and stopped first; asks you to type `delete` (not with `--yes`). Cannot be combined with `--keep-data` |
| `--all` | `-All` | Also runs the Cua Driver uninstaller (`cua.ai/driver/uninstall.sh` / `.ps1`), takes Cua Driver's skill and MCP server out of your agents (`cua agents remove --include-cua-driver`) and, on macOS, runs Lume's uninstaller (`cua.ai/lume/uninstall.sh`). Lume keeps your VMs unless `--delete-spaces` is given too |
| `--prefix DIR` | `-Prefix DIR` | Also removes what `install.sh --prefix DIR` put under `DIR/bin` and `DIR/Applications` |
| `--no-modify-path` | `-NoModifyPath` | Never edits your shell profile (user PATH on Windows) |
| `--dry-run` | `-DryRun` | Prints what would be removed, changes nothing. It runs `cua agents remove --dry-run` to list the agent skills and MCP entries one by one |
| `--yes` | `-Yes` | Accepts every prompt |

What it removes, in this order (each step is best effort and bounded, so one failure never stops the rest):

1. Quits the Cua Spaces app, lists the Spaces on this computer and stops the running ones (their disks stay).
2. `cua host remove --force --uninstalling`: unregisters this computer from the relay and removes the host service. When `cua` is not there to do it, it removes the service itself (launchd `com.trycua.spacesd.host` on macOS, the systemd unit `cua-spacesd-host` on Linux, the `Cua\SpacesdHost` scheduled task on Windows, and the names used before the rename).
3. `cua agents remove --agents all --yes`: the Cua Spaces skills and the `cua` MCP server (`cua-local` for Local) in your agents. The `cua-driver` skill and MCP server (`cua-driver-local` for Local) and every other tool's entries stay, unless `--all` adds `--include-cua-driver`. A `cua` older than the version with `--include-cua-driver` and `--dry-run` would also strip Cua Driver's entries, so the script skips this step, says what stays in the agents and how to remove it (install the latest `cua`, run `cua agents remove`), removes the rest and still exits 0; a dry run says so too. The record `cua agents remove` works from (`~/.cua/agent-setup.json`) stays unless `--delete-spaces`.
4. `cua daemon stop`, and the Cua Volume mount (`~/Cua Volume` on macOS, `~/cua-volume` on Linux) is unmounted before any file is deleted.
5. Signs out (`cua auth logout`, which revokes the session) and removes a leftover `credentials.json`.
6. The app: `Cua Spaces.app` in `/Applications` and `~/Applications` (macOS, only when its bundle id is Cua's); the `cua-spaces` package with apt (Linux .deb), the AppImage, its desktop entry and autostart entry; on Windows the app's own uninstaller (silent), which also removes launch at login. On macOS the app registered itself as a login item, which a script cannot unregister: with the app gone it has nothing to start, and System Settings, Login Items shows how to remove a leftover row.
7. The CLI: `~/.local/bin/cua`, `/usr/local/bin/cua` (and `DIR/bin/cua`), only when it is the Cua CLI (`cua --version` says `cua X.Y.Z`, or a link into the app). A `cua` from pip, npm or Homebrew is left alone and named. On Windows, `%LOCALAPPDATA%\Programs\cua\bin\cua.exe` and its user PATH entry.
8. The `# Added by the cua installer` PATH line in your shell profile (kept while `cua-driver` or `lume` still lives in the same folder).
9. Credentials: the macOS Keychain items of service `run.cua.ai` and `staging.run.cua.ai` (account `cua-cli` for the session; the device key and agent keys too unless `--keep-data`; this includes the legacy `run.cua.ai` label), and the Windows Credential Manager entries `cua-cli.*.run.cua.ai`. With `--delete-spaces` also the Keyvault master key (`Cua Keyvault`).
10. Settings and caches: in `~/.cua` the config, caches, telemetry, runtimes, skills, the host folder and the Cua Volume cache (`volume/cache`); on macOS the app's Application Support, Preferences, Caches, saved state and WebKit folders; on Linux and Windows the app's own settings folder. Everything else in `~/.cua` (sandboxes, vmm, images, `volume/data`, the Keyvault, trajectories, the Spaces registry) stays until you pass `--delete-spaces`.

The scripts are published next to the installers on the rolling `cua-install-latest` release (`release-installers-feed.yml`, and the CLI and app release workflows through `stamp_installers.py`), and cua.ai redirects `/uninstall.sh` and `/uninstall.ps1` to them the way it does `/install.sh`.

Tests: `python3 -m unittest discover -s scripts/install/tests -p 'test_uninstall.py'` (a throwaway HOME, recording fakes for `cua`, `launchctl`, `systemctl`, `security`, and an `rm` that refuses anything outside the sandbox) and the Pester file `tests/uninstall.Tests.ps1` (every step that touches the machine is mocked).

## Release manifest

Both scripts read `release-artifacts.json`:

- latest: `https://github.com/trycua/cua/releases/download/cua-install-latest/release-artifacts.json` (a rolling release the CD workflows update: `cd-cua-sdk.yml` moves the `cli` version after a cua SDK publish, `cd-cua-spaces.yml` the `app` version, and `release-installers-feed.yml` republishes install.sh / install.ps1 when they change on main)
- pinned: `https://github.com/trycua/cua/releases/download/cua-sdk-v<version>/release-artifacts.json`

The copies of `install.sh` / `install.ps1` on a repository's `cua-install-latest` release default to that repository: the release workflows publish them through `stamp_installers.py --repo "$GITHUB_REPOSITORY"`, so the trycua/cua-staging feed downloads, and verifies Sigstore identities against, trycua/cua-staging's own releases. The files in this directory keep `trycua/cua`.

`release_manifest.py` writes it from a directory of release files. Names it recognises:

| Component | File |
|---|---|
| CLI | `cua-cli-<ver>-<platform>.tar.gz` (macOS, Linux), `cua-cli-<ver>-windows-<arch>.zip` |
| App | `cua-spaces-<ver>-darwin-universal.dmg` / `.pkg`, `cua-spaces-<ver>-linux-<arch>.AppImage` / `.deb`, `cua-spaces-<ver>-windows-<arch>.msi` / `-setup.exe`. A stable release's Linux and Windows files are copies of the Electron installers (`Cua-Spaces-<ver>-x86_64.AppImage`, `cua-spaces_<ver>_amd64.deb`, `Cua-Spaces-Setup-<ver>-x64.exe`, ...) under these names; the electron-builder names stay on the release too, because the updater feed (`latest*.yml`) references them, and are listed with kinds `electron-*` |

`<platform>` is `darwin-arm64`, `darwin-x64`, `linux-x64`, `linux-arm64`, `windows-x64` or `windows-arm64`. Each entry carries `url`, `sha256` and, when present next to the file, `minisig` and `cosign_bundle` URLs. Every artifact sits on one line so install.sh can read it without jq.

Downloads always check sha256. Signatures are checked when the entry lists one and `minisign` (with `CUA_INSTALL_MINISIGN_PUBKEY` or the key baked into the script) or `cosign` is installed. If neither is installed the installer prints one info line (sha256 was checked) and goes on; `--require-signature` turns that into an error.

## Tests

```sh
scripts/install/tests/run.sh                     # install.sh against a local fake release
python3 -m unittest discover scripts/install/tests
scripts/install/tests/docker-pester.sh           # install.ps1 Pester tests in a container
```

`run.sh` uses a fake HOME and `--prefix` temp dirs; it never writes to your real home, `/Applications` or PATH. On macOS it also builds and mounts a throwaway .dmg. The `installer-smoke` job in `.github/workflows/ci-installers.yml` runs all of these on Linux, macOS and Windows.

Test hooks: `CUA_INSTALL_MANIFEST_URL`, `CUA_INSTALL_BASE_URL`, `CUA_INSTALL_DRIVER_URL` (the cua-driver installer, same HTTPS rules), `CUA_INSTALL_OS`, `CUA_INSTALL_ARCH`. With util-linux `script`, run.sh drives the checklist through a pty.
