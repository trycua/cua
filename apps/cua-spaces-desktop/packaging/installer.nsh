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
; src/model/environment.ts). The user's ~/.cua and app data stay, as with
; the SwiftUI app.

!include "FileFunc.nsh"

; electron-builder declares these only when no customCheckAppRunning is defined.
!include "getProcessInfo.nsh"
Var pid

!define CUA_SPACES_KEY "Software\ai.cua.spaces.desktop"
!define CUA_SPACES_RUN_NAME "ai.cua.spaces.desktop"

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
