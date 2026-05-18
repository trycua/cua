# Dev loop — cua-driver-rs on the Windows VM

Quick reference for iterating on Windows-specific cua-driver-rs code
directly on `fbonacci-windows-vm` without round-tripping through GitHub
CD. Set up Sun 2026-05-18 by the autonomous session for the doctor
segfault / Session-0 hang investigations.

## Connection

- **SSH** (Session 0 — services context, no desktop):
  ```bash
  ssh -i ~/.ssh/cua_winvm fbonacci@20.115.29.195
  ```
- **RDP** (Session 1+ with attached desktop, needed for visual / GUI tool validation):
  ```bash
  open /Users/francesco/Desktop/fbonacci-windows-vm.rdp
  ```
  Credentials: `fbonacci` + password from password manager.

## Toolchain on the VM (already installed)

- Git 2.54.0
- Rustup 1.29 + `stable-x86_64-pc-windows-msvc` (rustc 1.95.0)
- Cargo 1.95.0
- Visual Studio Build Tools 2022 (VCTools + Windows 11 SDK)
- Clone: `C:\Users\fbonacci\cua` (HEAD pinned to origin/main at clone time;
  `git pull` to refresh)

## Iteration loop

1. **Edit locally on the Mac** (use Claude Code, your IDE, etc.).
2. **Push changed files to the VM** via `scp`:
   ```bash
   scp -i ~/.ssh/cua_winvm \
     /Users/francesco/cua/libs/cua-driver-rs/crates/platform-windows/src/<file>.rs \
     fbonacci@20.115.29.195:cua/libs/cua-driver-rs/crates/platform-windows/src/<file>.rs
   ```
3. **Build on the VM** (~20 s incremental, ~5 min cold):
   ```bash
   ssh -i ~/.ssh/cua_winvm fbonacci@20.115.29.195 \
     "powershell -Command 'cd cua\\libs\\cua-driver-rs; cargo build --release -p cua-driver'"
   ```
4. **Test on the VM** — use PowerShell `Start-Job` for clean timeouts;
   pipe JSON args via stdin to bypass PowerShell's nested-quoting hell:
   ```powershell
   '{"name":"notepad"}' | & "$env:USERPROFILE\cua\libs\cua-driver-rs\target\release\cua-driver.exe" call launch_app
   ```

## Gotchas the autonomous session hit

1. **PowerShell heredoc quoting** — `Start-Process -ArgumentList` with a JSON
   arg containing `"` will silently strip the quotes. **Workaround:**
   pipe the JSON via stdin to `cua-driver call <tool>` instead of
   passing it as argv.

2. **`Start-Process -RedirectStandardError` buffering** — stderr from
   the child process may not flush to the redirect file until the child
   exits cleanly. If you `Stop-Process` mid-run you lose buffered
   stderr. **Workaround:** use `Start-Job` with `Tee-Object` for live
   capture, or write to a file from inside the script.

3. **PowerShell strings with `(`, `,`, em-dashes** — PowerShell parses
   parentheses and commas inside double-quoted strings as method calls
   in some contexts. Em-dashes get corrupted by `scp`. **Workaround:**
   stick to plain ASCII single-quoted strings in any script you `scp`
   over.

4. **`R` is a PowerShell alias for `Invoke-History`.** Don't name a
   helper function `R`.

5. **Some parity examples hard-code `target/debug/cua-driver.exe`** —
   if you've only run `cargo build --release` you need to also run
   `cargo build` (debug) once, otherwise `list_tools_parity` and
   `describe_parity` fail with "cannot find the path specified".

6. **`SetForegroundWindow` is restricted on Windows** — calling it
   from a non-foreground process silently no-ops most of the time. The
   foreground-restore in `launch_uwp` is best-effort. Plan visual
   confirmation in Session 1+ for any focus-related work.

7. **Visual Studio Build Tools install is ~5 GB and takes ~10-15 min
   over Azure VM network.** Don't reinstall — it's there.

## Running the parity suite

```powershell
$env:CUA_DRIVER_RS_TELEMETRY_ENABLED = "0"
$exe = "$env:USERPROFILE\cua\libs\cua-driver-rs\target\release\cua-driver.exe"

# 1. Start the daemon
Get-Process | Where-Object { $_.Name -eq 'cua-driver' } | Stop-Process -Force
Start-Process -FilePath $exe -ArgumentList 'serve' -NoNewWindow

# 2. Run all parity examples (skip overlay_dump — it's interactive-overlay-only)
cd $env:USERPROFILE\cua\libs\cua-driver-rs
$examples = @('list_tools_parity','describe_parity','get_screen_size_parity',
              'get_cursor_position_parity','config_parity',
              'agent_cursor_setters_parity','get_agent_cursor_state_parity',
              'set_agent_cursor_style_parity','list_windows_parity',
              'list_apps_parity','launch_app_parity')
foreach ($ex in $examples) {
    cargo run --release --quiet --example $ex -p platform-windows
    Write-Host "$ex exit=$LASTEXITCODE"
}

# 3. Cleanup
Get-Process | Where-Object { $_.Name -eq 'cua-driver' } | Stop-Process -Force
```

In Session 0 the autonomous session got 11/11 PASS as of 2026-05-18.

## Commands the autonomous session used most

```bash
# Quick file push
scp -i ~/.ssh/cua_winvm <local> fbonacci@20.115.29.195:cua/libs/cua-driver-rs/...

# Quick build + test with timeout (Mac side)
ssh -o ServerAliveInterval=20 -i ~/.ssh/cua_winvm fbonacci@20.115.29.195 \
  'powershell -ExecutionPolicy Bypass -File some-script.ps1'

# Quick PSCMD via stdin pipe
echo '{"name":"notepad"}' | ssh -i ~/.ssh/cua_winvm fbonacci@20.115.29.195 \
  '"C:\\Users\\fbonacci\\cua\\libs\\cua-driver-rs\\target\\release\\cua-driver.exe" call launch_app'

# Kill all stale cua-driver procs
ssh -i ~/.ssh/cua_winvm fbonacci@20.115.29.195 \
  "powershell -Command 'Get-Process cua-driver -EA SilentlyContinue | Stop-Process -Force'"

# Check which sessions exist
ssh -i ~/.ssh/cua_winvm fbonacci@20.115.29.195 "powershell -Command qwinsta"
```
