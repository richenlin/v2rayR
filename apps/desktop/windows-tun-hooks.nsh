; The application only elevates the bundle installed in the protected default directory.
!macro NSIS_HOOK_PREINSTALL
  StrCmp "$INSTDIR" "$PROGRAMFILES64\v2rayR" +2 0
  Abort "Windows TUN requires the default v2rayR installation directory in Program Files."
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; A split-token administrator can otherwise own the files under their user SID.
  nsExec::ExecToLog '"$SYSDIR\icacls.exe" "$INSTDIR" /setowner *S-1-5-32-544'
  Pop $0
  StrCmp $0 "0" +2 0
  Abort "Could not set the v2rayR installation owner."
  nsExec::ExecToLog '"$SYSDIR\icacls.exe" "$INSTDIR" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F *S-1-5-32-545:(OI)(CI)RX'
  Pop $0
  StrCmp $0 "0" +2 0
  Abort "Could not set the v2rayR installation permissions."
  nsExec::ExecToLog '"$SYSDIR\icacls.exe" "$INSTDIR\windows-tun" /reset /T'
  Pop $0
  StrCmp $0 "0" +2 0
  Abort "Could not reset the Windows TUN bundle permissions."
  nsExec::ExecToLog '"$SYSDIR\icacls.exe" "$INSTDIR\windows-tun" /setowner *S-1-5-32-544 /T'
  Pop $0
  StrCmp $0 "0" +2 0
  Abort "Could not set the Windows TUN bundle owner."
!macroend
