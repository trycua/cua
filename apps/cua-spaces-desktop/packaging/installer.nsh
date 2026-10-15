; SPDX-License-Identifier: FSL-1.1-MIT
; Copyright (c) 2026 Cua AI, Inc.

; The NSIS installer's and uninstaller's own steps (electron-builder `nsis.include`).
;
; The cua daemon runs from $INSTDIR\resources\native\cua.exe and outlives the
; app. Left running, an upgrade cannot replace cua.exe (Windows keeps a
; running executable's file) and an uninstall cannot remove the folder. Both
; programs check for the running app before they touch a file
; (CHECK_APP_RUNNING: the installer's section, before the old version's
; uninstaller runs and before the new files are written; the uninstaller's
; init). That check is replaced here by the same check followed by stopping
; the daemon, so the app is gone (and cannot start the daemon again) before
; the daemon is stopped, and the daemon is stopped before any file is written
; or removed.
;
; Uninstalling (not upgrading) also removes what the app added outside its
; folder: launch at login (the Run entry, src/login-item.ts) and the `cua`
; it put on PATH, when the app's record says it did (WINDOWS_APP_KEY,
; src/model/environment.ts), and turns off unattended access (CUA-1740): the
; host service (the per-user scheduled task Cua\SpacesdHost, or its logon
; entry, running ~/.cua/host/bin/cua-spacesd.exe) would otherwise keep the PC
; online and shared on the relay. An upgrade runs this uninstaller too (as
; --updated, so isUpdated holds) and keeps the host running. The user's ~/.cua (apart from
; ~/.cua/host, the service's own files) and app data stay, as with the
; SwiftUI app.

!include "FileFunc.nsh"

; electron-builder declares these only when no customCheckAppRunning is defined.
!include "getProcessInfo.nsh"
Var pid

!define CUA_SPACES_KEY "Software\ai.cua.spaces.desktop"
!define CUA_SPACES_RUN_NAME "ai.cua.spaces.desktop"
; cua-host's Windows service (libs/cua/crates/cua-host/src/service.rs).
!define CUA_HOST_TASK "Cua\SpacesdHost"
!define CUA_HOST_RUN_NAME "CuaSpacesdHost"

!macro customCheckAppRunning
  !insertmacro IS_POWERSHELL_AVAILABLE
  !insertmacro _CHECK_APP_RUNNING
  !insertmacro stopCuaDaemon
!macroend

; `cua daemon stop` with the installed cua, then anything still running from
; the install folder (a daemon that did not answer).
!macro stopCuaDaemon
  ${if} ${FileExists} "$INSTDIR\resources\native\cua.exe"
    DetailPrint "Stopping the cua daemon"
    nsExec::Exec /TIMEOUT=20000 '"$INSTDIR\resources\native\cua.exe" daemon stop'
    Pop $0
  ${endIf}
  ${if} $IsPowerShellAvailable == 0
    nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -C "Get-CimInstance -ClassName Win32_Process | ? {$$_.Path -and $$_.Path.StartsWith('$INSTDIR\', 'CurrentCultureIgnoreCase')} | % { Stop-Process -Id $$_.ProcessId -Force -ErrorAction SilentlyContinue }"`
    Pop $0
  ${endIf}
  ; Windows releases a stopped process's files a moment later.
  Sleep 500
!macroend

!macro customUnInstall
  ${ifNot} ${isUpdated}
    !insertmacro removeHostService
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${CUA_SPACES_RUN_NAME}"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "${CUA_SPACES_RUN_NAME}"
    !insertmacro removeAppCli
    DeleteRegKey HKCU "${CUA_SPACES_KEY}"
  ${endIf}
!macroend

; The `cua` the app copied onto PATH, its folder when empty, and the folder's
; user PATH entry when the app added it. The folder reaches PowerShell in the
; environment, so no quoting of the path is needed.
!macro removeAppCli
  ReadRegStr $0 HKCU "${CUA_SPACES_KEY}" "CliInstalled"
  ${if} $0 != ""
    Delete "$0"
    ${GetParent} "$0" $1
    RMDir "$1"
    ${GetParent} "$1" $2
    RMDir "$2"
  ${endIf}
  ReadRegStr $0 HKCU "${CUA_SPACES_KEY}" "CliPathAdded"
  ${if} $0 != ""
    ${ifNot} ${FileExists} "$0\cua.exe"
      System::Call 'Kernel32::SetEnvironmentVariable(t "CUA_SPACES_CLI_DIR", t r0)'
      nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -C "$$d = $$env:CUA_SPACES_CLI_DIR.TrimEnd('\'); $$p = [Environment]::GetEnvironmentVariable('Path', 'User'); if ($$p) { $$kept = @($$p -split ';' | ? { $$_ -and $$_.TrimEnd('\') -ne $$d }); [Environment]::SetEnvironmentVariable('Path', ($$kept -join ';'), 'User') }"`
      Pop $1
    ${endIf}
  ${endIf}
!macroend

; Unattended access, as the .deb's prerm.sh turns it off: `cua host remove
; --force --uninstalling` (a no-op on a machine never set up; it follows
; CUA_HOME) unregisters this machine and the Spaces it provides from the relay, deletes the scheduled task (or logon entry) and removes
; ~/.cua/host. Bounded, and best effort: an offline relay is reported by cua
; after the local cleanup and never stops the uninstall. Then, whatever it
; could not reach: the task, the logon entry, a driver still running from
; ~/.cua (or a CUA_HOME), and ~/.cua/host. cua may have started the daemon
; again to turn the provided Spaces off, so it is stopped once more before
; the app's files are removed.
!macro removeHostService
  ${if} ${FileExists} "$INSTDIR\resources\native\cua.exe"
    DetailPrint "Turning off unattended access"
    nsExec::Exec /TIMEOUT=90000 '"$INSTDIR\resources\native\cua.exe" host remove --force --uninstalling'
    Pop $0
    !insertmacro stopCuaDaemon
  ${endIf}
  nsExec::Exec '"$SYSDIR\schtasks.exe" /End /TN "${CUA_HOST_TASK}"'
  Pop $0
  nsExec::Exec '"$SYSDIR\schtasks.exe" /Delete /TN "${CUA_HOST_TASK}" /F'
  Pop $0
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${CUA_HOST_RUN_NAME}"
  ; The driver, and the conhost/cmd.exe that runs it and holds driver.log
  ; open (`>> driver.log`), with their children: killing only the driver
  ; left cmd.exe writing the log, so driver.log outlived the folder (CUA-1749).
  ${if} $IsPowerShellAvailable == 0
    nsExec::Exec `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -C "Get-CimInstance -ClassName Win32_Process | ? {$$_.ProcessId -ne $$PID -and (($$_.Path -and $$_.Path -like '*\host\bin\cua-spacesd.exe') -or ($$_.CommandLine -and ($$_.CommandLine -like '*\host\bin\cua-spacesd.exe*' -or $$_.CommandLine -like '*\host\driver.log*')))} | % { taskkill.exe /T /F /PID $$_.ProcessId 2>&1 | Out-Null }"`
    Pop $0
  ${endIf}
  !insertmacro removeHostFolder
!macroend

; ~/.cua/host, retried for up to 10 s: Windows releases a killed process's
; files (driver.log above all) a moment after it exits.
!macro removeHostFolder
  StrCpy $1 0
  ${Do}
    Sleep 500
    RMDir /r "$PROFILE\.cua\host"
    ${IfNot} ${FileExists} "$PROFILE\.cua\host\*.*"
      ${Break}
    ${EndIf}
    IntOp $1 $1 + 1
  ${LoopUntil} $1 >= 20
!macroend
