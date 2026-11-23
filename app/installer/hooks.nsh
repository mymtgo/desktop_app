; MyMTGO's Windows installer hooks (release spec section 13 and 13.2), named by
; bundle.windows.nsis.installerHooks in tauri.conf.json. Tauri 2.12 includes
; this file at the top of its installer.nsi; the macros run inside its
; install and uninstall sections. $PassiveMode (/P) and $UpdateMode (/UPDATE)
; are the template's own variables; the updater runs the installer with
; both.
;
; Windows only: no test can run it. app/tests/installer_hooks.rs pins the
; text, the owner runs it on a Windows VM (Plan W, W16 step 6).

!include LogicLib.nsh
!include FileFunc.nsh

Var Mymtgo0xIndex
Var Mymtgo0xKey
Var Mymtgo0xName
Var Mymtgo0xUninstaller
Var Mymtgo0xFolder
Var Mymtgo0xResult
Var Mymtgo0xData
Var MymtgoV1Data
Var MymtgoFirstRunChoice
Var MymtgoChoiceHandle

!macro NSIS_HOOK_PREINSTALL
  ; Nothing chosen yet: a question never asked writes no choice file.
  StrCpy $MymtgoFirstRunChoice ""
  ; Silent, passive and update installs never ask either question.
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode != 1
  ${AndIf} $UpdateMode != 1
    ; Section 13.2: migrate from 0.x or start fresh, asked only on a first
    ; install (no v1 database yet) with 0.x data at one of the importer's
    ; packaged places. The 0.x database is only checked for, never opened.
    ; v1 imports on its first start (app/src/first_run.rs), with progress.
    StrCpy $Mymtgo0xData 0
    StrCpy $MymtgoV1Data 0
    IfFileExists "$APPDATA\com.mymtgo.desktop\mymtgo.sqlite" 0 +2
      StrCpy $MymtgoV1Data 1
    IfFileExists "$APPDATA\mymtgo\database\database.sqlite" 0 +2
      StrCpy $Mymtgo0xData 1
    IfFileExists "$APPDATA\com.mymtgo.app\database\database.sqlite" 0 +2
      StrCpy $Mymtgo0xData 1
    ${If} $Mymtgo0xData == 1
    ${AndIf} $MymtgoV1Data == 0
      MessageBox MB_YESNO|MB_ICONQUESTION "MyMTGO 0.x data was found on this computer.$\r$\n$\r$\nYes: migrate from 0.x. MyMTGO imports your matches, decks and settings when it first opens.$\r$\nNo: start fresh. You can import later from the Import page." /SD IDNO IDYES mymtgo_choice_migrate IDNO mymtgo_choice_fresh
      mymtgo_choice_migrate:
        StrCpy $MymtgoFirstRunChoice "migrate"
        Goto mymtgo_choice_done
      mymtgo_choice_fresh:
        StrCpy $MymtgoFirstRunChoice "fresh"
      mymtgo_choice_done:
    ${EndIf}

    ; Item 5: offer to remove MyMTGO 0.x before v1 writes its shortcuts (0.x's
    ; "MyMTGO" shortcuts clash with v1's). Asked after the migration question.
    ; 0.x's uninstall entry: electron-builder writes it under a GUID key in
    ; the user's Uninstall list with DisplayName "mymtgo". S== is case
    ; sensitive, so v1's own "MyMTGO" entry never matches.
    StrCpy $Mymtgo0xUninstaller ""
    StrCpy $Mymtgo0xIndex 0
    ${Do}
      ClearErrors
      EnumRegKey $Mymtgo0xKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall" $Mymtgo0xIndex
      ${If} ${Errors}
      ${OrIf} $Mymtgo0xKey == ""
        ${Break}
      ${EndIf}
      ReadRegStr $Mymtgo0xName HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$Mymtgo0xKey" "DisplayName"
      ${If} $Mymtgo0xName S== "mymtgo"
        ReadRegStr $Mymtgo0xUninstaller HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\$Mymtgo0xKey" "UninstallString"
        ${Break}
      ${EndIf}
      IntOp $Mymtgo0xIndex $Mymtgo0xIndex + 1
    ${Loop}

    ; UninstallString is '"<folder>\Uninstall mymtgo.exe" /currentuser':
    ; keep the quoted path.
    StrCpy $Mymtgo0xName $Mymtgo0xUninstaller 1
    ${If} $Mymtgo0xName == '"'
      StrCpy $Mymtgo0xUninstaller $Mymtgo0xUninstaller "" 1
      StrCpy $Mymtgo0xIndex 0
      ${Do}
        StrCpy $Mymtgo0xName $Mymtgo0xUninstaller 1 $Mymtgo0xIndex
        ${If} $Mymtgo0xName == '"'
        ${OrIf} $Mymtgo0xName == ""
          ${Break}
        ${EndIf}
        IntOp $Mymtgo0xIndex $Mymtgo0xIndex + 1
      ${Loop}
      StrCpy $Mymtgo0xUninstaller $Mymtgo0xUninstaller $Mymtgo0xIndex
    ${EndIf}

    ; The path comes from the registry, and FileExists and Delete accept
    ; wildcards: only electron-builder's "Uninstall mymtgo.exe" (LogicLib's
    ; == ignores case).
    ${GetFileName} $Mymtgo0xUninstaller $Mymtgo0xName
    ${If} $Mymtgo0xUninstaller != ""
    ${AndIf} ${FileExists} $Mymtgo0xUninstaller
    ${AndIf} $Mymtgo0xName == "Uninstall mymtgo.exe"
      ${GetParent} $Mymtgo0xUninstaller $Mymtgo0xFolder
      MessageBox MB_YESNO|MB_ICONQUESTION "MyMTGO 0.x is installed. Remove it now? Its data stays on this computer, so you can import it into MyMTGO." /SD IDNO IDYES mymtgo_0x_remove IDNO mymtgo_0x_keep
      mymtgo_0x_remove:
        ; Silent, and never with --delete-app-data: 0.x's database in
        ; %APPDATA%\mymtgo stays for the import (0.x's
        ; deleteAppDataOnUninstall is false). _?= runs the uninstaller in
        ; place, so ExecWait waits for it (an NSIS uninstaller otherwise
        ; copies itself to TEMP and returns at once); it then leaves its
        ; own file behind. ExecWait sets the error flag, not the result,
        ; when it cannot run.
        ClearErrors
        ExecWait '"$Mymtgo0xUninstaller" /currentuser /S _?=$Mymtgo0xFolder' $Mymtgo0xResult
        ${If} $Mymtgo0xResult == 0
        ${AndIfNot} ${Errors}
          ; Only the leftover uninstaller, and the folder if now empty.
          Delete $Mymtgo0xUninstaller
          RMDir $Mymtgo0xFolder
        ${EndIf}
      mymtgo_0x_keep:
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Section 13.2: the install succeeded, so the answer to the migration
  ; question is recorded as one word for v1's first start, which reads and
  ; deletes the file. Unanswered (silent, passive, update, no 0.x data, a v1
  ; database already there): no file.
  ${If} $MymtgoFirstRunChoice != ""
    CreateDirectory "$APPDATA\com.mymtgo.desktop"
    ClearErrors
    FileOpen $MymtgoChoiceHandle "$APPDATA\com.mymtgo.desktop\first-run-choice.txt" w
    ${IfNot} ${Errors}
      FileWrite $MymtgoChoiceHandle $MymtgoFirstRunChoice
      FileClose $MymtgoChoiceHandle
    ${EndIf}
  ${EndIf}

  ; Section 13.1: the first install under the new name removes the beta's
  ; install (its folder, uninstall entry and shortcuts are named after the
  ; product) and moves its launch at login. Data stays: both names share
  ; %APPDATA%\com.mymtgo.desktop (the identifier). Tauri 2.12's uninstaller
  ; deletes that folder only when its confirm page's "Delete the application
  ; data" box was ticked ($DeleteAppDataCheckboxState, set in
  ; un.ConfirmLeave); a silent run (/S) skips every page and no command line
  ; option sets it. Runs on every kind of install (the beta's updater runs
  ; this installer with /UPDATE), after this install succeeded.
  ;
  ; The guard sits here, not at the top of the file: Tauri 2.12 includes
  ; this file before it defines PRODUCTNAME, so a top level `!if` would
  ; compare the literal text and always hold. A macro expands where it is
  ; inserted (Section Install, after the define), so the last beta compiles
  ; none of this, its variables included (`Var /GLOBAL` is allowed here).
  !if "${PRODUCTNAME}" != "MyMTGO Beta"
    Var /GLOBAL MymtgoBetaUninstaller
    Var /GLOBAL MymtgoBetaFolder
    Var /GLOBAL MymtgoBetaResult
    Var /GLOBAL MymtgoBetaQuote
    Var /GLOBAL MymtgoBetaAutostart
    Var /GLOBAL MymtgoBetaAutostartValue
    Var /GLOBAL MymtgoBetaStartMenu
    Var /GLOBAL MymtgoBetaDesktop
    Var /GLOBAL MymtgoBetaApproved
    Var /GLOBAL MymtgoBetaOutput
    Var /GLOBAL MymtgoBetaIndex
    Var /GLOBAL MymtgoBetaChar
    Var /GLOBAL MymtgoBetaExit
    Var /GLOBAL MymtgoBetaLength
    Var /GLOBAL MymtgoBetaExe
    Var /GLOBAL MymtgoBetaShort
    Var /GLOBAL MymtgoBetaShortInstall
    Var /GLOBAL MymtgoBetaSameFolder

    ; The beta's uninstaller deletes its own Run value, so read it first.
    StrCpy $MymtgoBetaAutostart 0
    ClearErrors
    ReadRegStr $MymtgoBetaAutostartValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MyMTGO Beta"
    ${IfNot} ${Errors}
      StrCpy $MymtgoBetaAutostart 1
    ${EndIf}

    ; Task Manager's Startup tab keeps its on/off choice in a binary value of
    ; the same name under StartupApproved\Run: a first byte with its low bit
    ; set (03) means the user switched the beta off. Read through reg.exe
    ; (NSIS cannot read a binary value), before anything is deleted. No
    ; value, or any other answer, counts as on.
    StrCpy $MymtgoBetaApproved 1
    ClearErrors
    nsExec::ExecToStack '"$SYSDIR\reg.exe" query "HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" /v "MyMTGO Beta"'
    Pop $MymtgoBetaExit
    Pop $MymtgoBetaOutput
    ${If} $MymtgoBetaExit == 0
      ; The line reads "MyMTGO Beta    REG_BINARY    0300...": the first
      ; byte's two hex digits follow the type and its spaces.
      StrLen $MymtgoBetaLength $MymtgoBetaOutput
      StrCpy $MymtgoBetaIndex 0
      ${Do}
        ${If} $MymtgoBetaIndex >= $MymtgoBetaLength
          ${Break}
        ${EndIf}
        StrCpy $MymtgoBetaChar $MymtgoBetaOutput 10 $MymtgoBetaIndex
        ${If} $MymtgoBetaChar == "REG_BINARY"
          IntOp $MymtgoBetaIndex $MymtgoBetaIndex + 10
          ${Do}
            StrCpy $MymtgoBetaChar $MymtgoBetaOutput 1 $MymtgoBetaIndex
            ${If} $MymtgoBetaChar != " "
              ${Break}
            ${EndIf}
            IntOp $MymtgoBetaIndex $MymtgoBetaIndex + 1
          ${Loop}
          ; The low nibble is the byte's second digit.
          IntOp $MymtgoBetaIndex $MymtgoBetaIndex + 1
          StrCpy $MymtgoBetaChar $MymtgoBetaOutput 1 $MymtgoBetaIndex
          IntOp $MymtgoBetaChar "0x$MymtgoBetaChar" & 1
          ${If} $MymtgoBetaChar == 1
            StrCpy $MymtgoBetaApproved 0
          ${EndIf}
          ${Break}
        ${EndIf}
        IntOp $MymtgoBetaIndex $MymtgoBetaIndex + 1
      ${Loop}
    ${EndIf}
    ClearErrors

    ; The beta's shortcuts, which its uninstaller removes. An update
    ; (/UPDATE) makes none for the new name, so each one is made below.
    StrCpy $MymtgoBetaStartMenu 0
    StrCpy $MymtgoBetaDesktop 0
    IfFileExists "$SMPROGRAMS\MyMTGO Beta.lnk" 0 +2
      StrCpy $MymtgoBetaStartMenu 1
    IfFileExists "$DESKTOP\MyMTGO Beta.lnk" 0 +2
      StrCpy $MymtgoBetaDesktop 1

    ; The beta's own executable name, which its Uninstall key records (the
    ; template writes MainBinaryName there; the new build may name its exe
    ; differently). Read now: the beta's uninstaller deletes the key. An
    ; empty value, or one with a path separator or wildcard, falls back to
    ; the new build's name, which is never deleted below.
    ReadRegStr $MymtgoBetaExe HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MyMTGO Beta" "MainBinaryName"
    ${If} $MymtgoBetaExe == ""
      StrCpy $MymtgoBetaExe "${MAINBINARYNAME}.exe"
    ${EndIf}
    StrLen $MymtgoBetaLength $MymtgoBetaExe
    StrCpy $MymtgoBetaIndex 0
    ${Do}
      ${If} $MymtgoBetaIndex >= $MymtgoBetaLength
        ${Break}
      ${EndIf}
      StrCpy $MymtgoBetaChar $MymtgoBetaExe 1 $MymtgoBetaIndex
      ${If} $MymtgoBetaChar == "\"
      ${OrIf} $MymtgoBetaChar == "*"
      ${OrIf} $MymtgoBetaChar == "?"
      ${OrIf} $MymtgoBetaChar == "/"
      ${OrIf} $MymtgoBetaChar == ":"
        StrCpy $MymtgoBetaExe "${MAINBINARYNAME}.exe"
        ${Break}
      ${EndIf}
      IntOp $MymtgoBetaIndex $MymtgoBetaIndex + 1
    ${Loop}
    ; Only an .exe, and never the uninstaller (LogicLib compares ignoring
    ; case).
    StrCpy $MymtgoBetaChar $MymtgoBetaExe 4 -4
    ${If} $MymtgoBetaChar != ".exe"
    ${OrIf} $MymtgoBetaExe == "uninstall.exe"
      StrCpy $MymtgoBetaExe "${MAINBINARYNAME}.exe"
    ${EndIf}

    ; UninstallString is '"<folder>\uninstall.exe"': keep the quoted path.
    ReadRegStr $MymtgoBetaUninstaller HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MyMTGO Beta" "UninstallString"
    StrCpy $MymtgoBetaQuote $MymtgoBetaUninstaller 1
    ${If} $MymtgoBetaQuote == '"'
      StrCpy $MymtgoBetaUninstaller $MymtgoBetaUninstaller "" 1
      StrCpy $MymtgoBetaQuote $MymtgoBetaUninstaller "" -1
      ${If} $MymtgoBetaQuote == '"'
        StrCpy $MymtgoBetaUninstaller $MymtgoBetaUninstaller -1
      ${EndIf}
    ${EndIf}

    ; Only Tauri's "uninstall.exe", as for 0.x above.
    ${GetFileName} $MymtgoBetaUninstaller $MymtgoBetaQuote
    ${If} $MymtgoBetaUninstaller != ""
    ${AndIf} ${FileExists} $MymtgoBetaUninstaller
    ${AndIf} $MymtgoBetaQuote == "uninstall.exe"
      ${GetParent} $MymtgoBetaUninstaller $MymtgoBetaFolder
      ; Never this install's own folder. Compared as 8.3 paths, so a long
      ; name, a short name or a trailing backslash for one folder still
      ; match. A path that cannot be resolved leaves the beta untouched:
      ; neither its uninstaller nor the same-folder cleanup runs.
      ; 0 different, 1 same, 2 unknown.
      StrCpy $MymtgoBetaSameFolder 1
      ClearErrors
      GetFullPathName /SHORT $MymtgoBetaShort $MymtgoBetaFolder
      GetFullPathName /SHORT $MymtgoBetaShortInstall $INSTDIR
      ; No trailing backslash on either.
      StrCpy $MymtgoBetaChar $MymtgoBetaShort 1 -1
      ${If} $MymtgoBetaChar == "\"
        StrCpy $MymtgoBetaShort $MymtgoBetaShort -1
      ${EndIf}
      StrCpy $MymtgoBetaChar $MymtgoBetaShortInstall 1 -1
      ${If} $MymtgoBetaChar == "\"
        StrCpy $MymtgoBetaShortInstall $MymtgoBetaShortInstall -1
      ${EndIf}
      ${If} $MymtgoBetaShort == ""
      ${OrIf} $MymtgoBetaShortInstall == ""
        StrCpy $MymtgoBetaSameFolder 2
      ${ElseIf} $MymtgoBetaShort != $MymtgoBetaShortInstall
        StrCpy $MymtgoBetaSameFolder 0
      ${EndIf}
      ClearErrors
      ${If} $MymtgoBetaSameFolder == 0
        ; Silent and in place (_?= makes ExecWait wait, as for 0.x above).
        ; No /UPDATE: the beta's shortcuts and Run value go too.
        StrCpy $MymtgoBetaResult "error"
        ; ExecWait sets the error flag, not the result, when it cannot run.
        ClearErrors
        ExecWait '"$MymtgoBetaUninstaller" /S _?=$MymtgoBetaFolder' $MymtgoBetaResult
        ${If} $MymtgoBetaResult == 0
        ${AndIfNot} ${Errors}
          ; Only the leftover uninstaller, and the folder if now empty.
          Delete $MymtgoBetaUninstaller
          RMDir $MymtgoBetaFolder
        ${EndIf}
      ${ElseIf} $MymtgoBetaSameFolder == 1
        ; The beta lives in this install's own folder, so its uninstaller is
        ; never run (it would delete what was just installed). Its uninstall
        ; entry, its shortcuts that target the beta's executable here and
        ; that executable (when it is not this build's) are removed instead:
        ; the entry would offer to uninstall this install under the beta's
        ; name.
        DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\MyMTGO Beta"
        ${If} $MymtgoBetaStartMenu == 1
          !insertmacro IsShortcutTarget "$SMPROGRAMS\MyMTGO Beta.lnk" "$INSTDIR\$MymtgoBetaExe"
          Pop $MymtgoBetaExit
          ${If} $MymtgoBetaExit = 1
            !insertmacro UnpinShortcut "$SMPROGRAMS\MyMTGO Beta.lnk"
            Delete "$SMPROGRAMS\MyMTGO Beta.lnk"
          ${EndIf}
        ${EndIf}
        ${If} $MymtgoBetaDesktop == 1
          !insertmacro IsShortcutTarget "$DESKTOP\MyMTGO Beta.lnk" "$INSTDIR\$MymtgoBetaExe"
          Pop $MymtgoBetaExit
          ${If} $MymtgoBetaExit = 1
            !insertmacro UnpinShortcut "$DESKTOP\MyMTGO Beta.lnk"
            Delete "$DESKTOP\MyMTGO Beta.lnk"
          ${EndIf}
        ${EndIf}
        ; The beta's own executable, when its name differs from this
        ; build's (the template's cleanup reads the new key, empty on a
        ; first install). A plain file name only.
        ${If} $MymtgoBetaExe != ""
        ${AndIf} $MymtgoBetaExe != "${MAINBINARYNAME}.exe"
          Delete "$INSTDIR\$MymtgoBetaExe"
        ${EndIf}
      ${EndIf}
    ${EndIf}

    ; The template's shortcuts, as it makes them on a first install, only
    ; on an update (a non-update install made them, or the user chose not
    ; to) and never with /NS. The Start menu path assumes no startMenuFolder
    ; in tauri.conf.json, as the template does without one. Made even when
    ; the file is there, as a first install does: 0.x's shortcut is
    ; mymtgo.lnk, the same file to Windows (names ignore case), and would
    ; otherwise leave Start opening 0.x.
    ${If} $MymtgoBetaStartMenu == 1
    ${AndIf} $UpdateMode = 1
    ${AndIf} $NoShortcutMode <> 1
      CreateShortcut "$SMPROGRAMS\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$SMPROGRAMS\${PRODUCTNAME}.lnk"
    ${EndIf}
    ${If} $MymtgoBetaDesktop == 1
    ${AndIf} $UpdateMode = 1
    ${AndIf} $NoShortcutMode <> 1
      CreateShortcut "$DESKTOP\${PRODUCTNAME}.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
      !insertmacro SetLnkAppUserModelId "$DESKTOP\${PRODUCTNAME}.lnk"
    ${EndIf}

    ; Launch at login under the new name, in the autostart plugin's format
    ; (auto-launch 0.6: the program path, a space, then no arguments).
    ; A beta switched off in Task Manager stays off: nothing is written.
    ${If} $MymtgoBetaAutostart == 1
      DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "MyMTGO Beta"
      ${If} $MymtgoBetaApproved == 1
        WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${PRODUCTNAME}" "$INSTDIR\${MAINBINARYNAME}.exe "
      ${EndIf}
    ${EndIf}

    ; What the beta left behind: Task Manager's on/off value under its name,
    ; and the install folder key the beta's installer wrote (its uninstaller
    ; removes it when it ran). Neither holds user data.
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "MyMTGO Beta"
    DeleteRegKey HKCU "Software\${MANUFACTURER}\MyMTGO Beta"
  !endif
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Endstep support: MyMTGO registers its browser helper with Chrome, Edge
  ; and Firefox on every start (app/src/endstep/host.rs). The installer puts
  ; the helper beside MyMTGO (a Tauri sidecar, removed with the other files);
  ; MyMTGO copies it into the data folder and registers that copy. This
  ; removes the registry keys.
  ; An update's uninstall keeps them (the new build writes them again).
  ${If} $UpdateMode <> 1
    DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\com.mymtgo.endstep"
    DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\com.mymtgo.endstep"
    DeleteRegKey HKCU "Software\Mozilla\NativeMessagingHosts\com.mymtgo.endstep"
  ${EndIf}
!macroend
