# Development Guide

This guide will help you set up your development environment and understand the process for contributing code to lume.

## Environment Setup

Lume development requires:

- Swift 6 or higher
- Xcode 15 or higher
- macOS Sequoia 15.2 or higher
- (Optional) VS Code with Swift extension

## Setting Up the Repository Locally

1. **Fork the Repository**: Create your own fork of lume
2. **Clone the Repository**:
   ```bash
   git clone https://github.com/trycua/lume.git
   cd lume
   ```
3. **Install Dependencies**:
   ```bash
   swift package resolve
   ```
4. **Build the Project**:
   ```bash
   swift build
   ```

## Development Workflow

1. Create a new branch for your changes
2. Make your changes
3. Run the tests: `swift test`
4. Build and test your changes locally
5. Commit your changes with clear commit messages

## Running a Source Build Beside the Released Lume (`lume-local`)

`scripts/install-local.sh` builds Lume from your checkout and installs it as `lume-local`, so you can run it while the released `lume` keeps serving. The two share no file, LaunchAgent, port or log:

| | Released | `lume-local` |
| --- | --- | --- |
| Command | `~/.local/bin/lume` | `~/.local/bin/lume-local` |
| Files | `~/.local/share/lume` | `~/.local/share/lume-local` |
| LaunchAgent | `com.trycua.lume_daemon` | `com.trycua.lume_daemon.local` |
| Port | 7777 | 7787 |
| Home and VMs | `~/.lume` | `~/.lume-local` |
| Settings | `~/.config/lume/config.yaml` | `~/.lume-local/config.yaml` |
| Daemon logs | `/tmp/lume_daemon.log` | `/tmp/lume_daemon.local.log` (or `$LUME_LOCAL_LOG_DIR`) |
| `run --detach` logs | `~/Library/Logs/lume/<vm>.log` | `~/Library/Logs/lume-local/<vm>.log` |

```bash
libs/lume/scripts/install-local.sh            # build and install
lume-local serve                              # listens on 127.0.0.1:7787, VMs in ~/.lume-local
libs/lume/scripts/uninstall-local.sh          # remove it; asks before deleting VMs (--yes, --keep-vms)
```

A binary named `lume-local` implies these defaults, like `cua-driver-local`. Explicit flags and environment still win: `lume-local serve --port 9000`, and for `lume-local`, `LUME_HOME=/path` moves its home, VM directory, settings file, image cache, release-channel file and telemetry identity. `lume-local` has its own home, so it ignores `XDG_CONFIG_HOME`, which every Lume on the Mac shares. The released `lume` is unchanged: `LUME_HOME` still only moves its release-channel file. `lume-local` never updates itself: `update` would run the installer of the released Lume.

`lume-local` uses `LUME_HOME` exactly as given, whitespace included (it is never trimmed into another path), and the scripts refuse a value with leading or trailing whitespace.

The `lume-local` command is a small wrapper that sets `LUME_HOME` to the Local home and `LUME_LOCAL_LOG_DIR` to the daemon's log directory, overriding any values it inherits (often the released Lume's), so the command line and the daemon always agree. The LaunchAgent gets the same two variables, so `lume-local logs` and the logs API read where the daemon writes, also when `LUME_LOCAL_LOG_DIR` moved the logs away from `/tmp`. The installer reads a custom home from `LUME_LOCAL_HOME` (it ignores `LUME_HOME`, which may be the released Lume's) and refuses port 7777. Give `uninstall-local.sh` the same `LUME_LOCAL_HOME`, `LUME_LOCAL_INSTALL_DIR` and `LUME_LOCAL_LOG_DIR` you gave the installer.

Both scripts canonicalise every path they write or delete (`.`, `..` and symlinks resolved) and refuse any that equals, sits inside, or contains a path the released Lume uses (`~/.lume`, `~/.config/lume`, `~/.local/bin/lume`, `~/.local/share/lume`, the stable plist, daemon logs and `~/Library/Logs/lume`). `$HOME/./.lume`, `$HOME/x/../.lume` and a symlink to `~/.lume` are all refused, so `uninstall-local.sh --yes` cannot delete the released Lume's VMs.

Older versions of `install-local.sh` installed over the released `lume`; those files are not touched, so reinstall the released Lume with `install.sh` if you still have one of those builds.

`scripts/tests/local-install-isolation-test.sh` runs both scripts against a fake home with stubbed `swift`, `codesign` and `launchctl`, and checks that nothing of the released Lume changes. `swift test` runs it.

## Submitting Pull Requests

1. Push your changes to your fork
2. Open a Pull Request with:
   - A clear title and description
   - Reference to any related issues
   - Screenshots or logs if relevant
3. Respond to any feedback from maintainers

## Testing Offline Unattended Setup

Use a local Apple restore image to exercise the vanilla macOS flow end to end:

```bash
lume create macos-tahoe --ipsw ~/Downloads/macos-tahoe.ipsw --unattended tahoe
lume run macos-tahoe
lume ssh macos-tahoe 'id -un'
```

The guest should report `lume`, accept SSH with the default `lume` password,
and boot without Setup Assistant. Tahoe is the currently verified preset;
Sequoia's remaining Accessibility-step issue is tracked in
[#2155](https://github.com/trycua/cua/issues/2155).
