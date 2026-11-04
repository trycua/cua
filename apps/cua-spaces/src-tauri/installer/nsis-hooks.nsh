; Cua Spaces NSIS installer hooks (bundle.windows.nsis.installerHooks).
;
; Silent / MDM installs preselect the first-run choice:
;   Cua-Spaces_x64-setup.exe /S /MODE=host     (set up for unattended access)
;   Cua-Spaces_x64-setup.exe /S /MODE=client   (access other machines)
; The mode is written to %USERPROFILE%\.cua\spaces-install-mode, which the app
; reads on first launch to preselect its onboarding screen. The app still asks
; before installing anything; nothing here installs a service.

!macro NSIS_HOOK_POSTINSTALL
  Push $R0
  Push $R1
  Push $R2
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/MODE=" $R1
  ${IfNot} ${Errors}
    ${If} $R1 == "host"
    ${OrIf} $R1 == "client"
      CreateDirectory "$PROFILE\.cua"
      FileOpen $R2 "$PROFILE\.cua\spaces-install-mode" w
      FileWrite $R2 "$R1"
      FileClose $R2
    ${EndIf}
  ${EndIf}
  Pop $R2
  Pop $R1
  Pop $R0
!macroend
