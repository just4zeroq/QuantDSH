; NSIS installer hooks for the QuantDSH carrier.
;
; The rule this preserves is the #469 fix: an upgrade must stop exactly the
; application process, never the unrelated helpers below $INSTDIR (the bundled
; Node/Python runtimes the Host spawns for a workspace).
;
; The handoff has three stages, in this order:
;   1. Ask the running instance to quit through the single-instance channel
;      (`QuantDSH.exe --dsh-installer-quit`), which performs the same orderly
;      `before-quit` choreography as a user-initiated quit.
;   2. Wait up to 30 seconds for it to finish disposing the workspace.
;   3. Escalate to a forced close of the exact image name, with a retry prompt.
;
; Tauri's own `CheckIfAppIsRunning` runs immediately after this hook (before the
; files are copied) and covers anything left over.
;
; A running process keeps its executable image file locked for writing, so
; `FileOpen ... a` is used as the process probe: an unlocked image means no
; instance of this exact binary is running. This avoids enumerating processes and
; therefore can never match a different executable.
;
; The messages are written literally instead of using Tauri's `$(appRunningOkKill)`
; / `$(failedToKillApp)` language strings: those embed a `{{product_name}}`
; placeholder that only Tauri's own template macros substitute, and this hook must
; stay compilable regardless of include order.

!macro NSIS_HOOK_PREINSTALL
  IfFileExists "$INSTDIR\${MAINBINARYNAME}.exe" 0 quantdsh_installer_app_absent
  FileOpen $R9 "$INSTDIR\${MAINBINARYNAME}.exe" a
  ${If} $R9 == ""
    ; Locked: an instance of this exact binary is running.
    Goto quantdsh_installer_app_running
  ${EndIf}
  FileClose $R9
  Goto quantdsh_installer_app_stopped

  quantdsh_installer_app_running:
  ; Newer versions receive this through the single-instance channel. Releases
  ; that do not implement the argument simply start, find the lock held and
  ; exit, so the scoped fallback below still covers them.
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --dsh-installer-quit'
  StrCpy $R8 0

  quantdsh_installer_wait_for_exit:
    FileOpen $R9 "$INSTDIR\${MAINBINARYNAME}.exe" a
    ${If} $R9 == ""
      Goto quantdsh_installer_wait_next
    ${EndIf}
    FileClose $R9
    Goto quantdsh_installer_app_stopped

  quantdsh_installer_wait_next:
    IntOp $R8 $R8 + 1
    ; Slow disks, antivirus hooks, and a large physical runtime can keep the
    ; process alive after disposal begins. Give the orderly handoff a full
    ; 30 seconds before escalating to the scoped forced-close path.
    ${If} $R8 < 60
      Sleep 500
      Goto quantdsh_installer_wait_for_exit
    ${EndIf}

  ; Scoped forced close of exactly the main binary. `taskkill /IM` matches the
  ; precise image name; it never reaches the other executables below $INSTDIR.
  MessageBox MB_OKCANCEL|MB_ICONEXCLAMATION "QuantDSH 正在运行。$\r$\n$\r$\n点击「确定」将强制关闭 QuantDSH 后继续安装；点击「取消」退出安装。" /SD IDOK IDOK quantdsh_installer_stop_app
  Quit

  quantdsh_installer_stop_app:
    ExecWait 'taskkill /F /IM "${MAINBINARYNAME}.exe"'
    Sleep 500
    StrCpy $R8 0

  quantdsh_installer_wait_for_fallback:
    FileOpen $R9 "$INSTDIR\${MAINBINARYNAME}.exe" a
    ${If} $R9 == ""
      Goto quantdsh_installer_fallback_next
    ${EndIf}
    FileClose $R9
    Goto quantdsh_installer_app_stopped

  quantdsh_installer_fallback_next:
    IntOp $R8 $R8 + 1
    ${If} $R8 > 1
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "无法关闭运行中的 QuantDSH。$\r$\n$\r$\n请手动退出后重试，或点击「取消」退出安装。" /SD IDCANCEL IDRETRY quantdsh_installer_wait_for_fallback
      Quit
    ${EndIf}
    Sleep 1000
    ExecWait 'taskkill /F /IM "${MAINBINARYNAME}.exe"'
    Sleep 500
    Goto quantdsh_installer_wait_for_fallback

  quantdsh_installer_app_stopped:
  quantdsh_installer_app_absent:
!macroend
