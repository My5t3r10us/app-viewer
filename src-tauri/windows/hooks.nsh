; Hooks of the Tauri NSIS installer (bundle > windows > nsis > installerHooks).
; appv.exe is installed next to app-viewer.exe; it adds / removes that folder in the user PATH,
; and the rule + skill that let Claude Code and Codex use it.

!macro NSIS_HOOK_POSTINSTALL
  nsExec::Exec '"$INSTDIR\appv.exe" install-path'
  nsExec::Exec '"$INSTDIR\appv.exe" agents install'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec '"$INSTDIR\appv.exe" uninstall-path'
  nsExec::Exec '"$INSTDIR\appv.exe" agents uninstall'
!macroend
