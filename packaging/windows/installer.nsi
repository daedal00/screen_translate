; Per-user installer for Screen Translate. Mirrors the layout of the Tauri NSIS installer (v0.3.1) so the
; v0.3.1 updater (`setup.exe /P /UPDATE /R /ARGS ...`) upgrades in place: same INSTDIR, same HKCU keys,
; same exe name.
;   makensis /DVERSION=<x.y.z> /DSRC=<path to the built exe> [/DOUTFILE=<installer path>] installer.nsi
Unicode true
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  !error "pass /DVERSION=<x.y.z>"
!endif
!ifndef SRC
  !error "pass /DSRC=<path to the release exe>"
!endif
!ifndef OUTFILE
  !define OUTFILE "Screen.Translate_${VERSION}_x64-setup.exe"
!endif

!define NAME "Screen Translate"
!define EXE "screen-translate.exe"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${NAME}"
!define PRODKEY "Software\screentranslate\${NAME}"
!define RUNKEY "Software\Microsoft\Windows\CurrentVersion\Run"

Name "${NAME}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
SetCompressor /SOLID lzma
Icon "..\icons\icon.ico"
UninstallIcon "..\icons\icon.ico"
InstallDir "$LOCALAPPDATA\${NAME}"
InstallDirRegKey HKCU "${PRODKEY}" ""

!insertmacro GetParameters
!insertmacro GetOptions
!insertmacro un.GetParameters
!insertmacro un.GetOptions

Var Passive
Var Update
Var Relaunch
Var Args
Var RemovedMsi

Page directory DirectoryPre
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Function DirectoryPre
  ${If} $Passive = 1
    Abort
  ${EndIf}
FunctionEnd

Function .onInit
  StrCpy $Passive 0
  StrCpy $Update 0
  StrCpy $Relaunch 0
  StrCpy $RemovedMsi 0
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/P" $R1
  ${IfNot} ${Errors}
    StrCpy $Passive 1
    SetAutoClose true
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/UPDATE" $R1
  ${IfNot} ${Errors}
    StrCpy $Update 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/R" $R1
  ${IfNot} ${Errors}
    StrCpy $Relaunch 1
  ${EndIf}
  ClearErrors
  ${GetOptions} $R0 "/ARGS" $Args
  ClearErrors
FunctionEnd

; v0.3.1 also shipped a per-machine MSI. Installing beside it would leave two copies, so remove it first.
Function RemoveMsi
  SetRegView 64
  StrCpy $R0 0
  ${Do}
    EnumRegKey $R1 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall" $R0
    ${If} $R1 == ""
      ${Break}
    ${EndIf}
    IntOp $R0 $R0 + 1
    ReadRegStr $R2 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\$R1" "DisplayName"
    ReadRegDWORD $R3 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\$R1" "WindowsInstaller"
    ${If} $R2 == "${NAME}"
    ${AndIf} $R3 = 1
      DetailPrint "Removing the previous MSI install"
      ClearErrors
      ExecWait 'msiexec /x $R1 /passive' $R4
      ; 3010 and 1641: removed, reboot needed or started. 1605: a stale entry whose product is already gone.
      ${If} ${Errors}
      ${OrIf} $R4 <> 0
        ${If} $R4 <> 3010
        ${AndIf} $R4 <> 1641
        ${AndIf} $R4 <> 1605
          SetRegView 32
          MessageBox MB_OK|MB_ICONSTOP "The previous MSI install of ${NAME} could not be removed (msiexec exit code $R4). Uninstall it from Settings > Apps, then run this installer again." /SD IDOK
          Abort
        ${EndIf}
      ${EndIf}
      ${If} $R4 <> 1605
        StrCpy $RemovedMsi 1
      ${EndIf}
      ${Break}
    ${EndIf}
  ${Loop}
  SetRegView 32
FunctionEnd

; The old exe may still be exiting (the v0.3.1 updater calls exit right after launching us).
Function ReleaseExe
  ${IfNot} ${FileExists} "$INSTDIR\${EXE}"
    Return
  ${EndIf}
  StrCpy $R0 0
  ${Do}
    ClearErrors
    Delete "$INSTDIR\${EXE}"
    ${IfNot} ${FileExists} "$INSTDIR\${EXE}"
      Return
    ${EndIf}
    ${If} $R0 >= 10
      ${Break}
    ${EndIf}
    IntOp $R0 $R0 + 1
    Sleep 500
  ${Loop}
  nsExec::Exec 'taskkill /F /IM ${EXE}'
  Pop $R1
  Sleep 1000
  Delete "$INSTDIR\${EXE}"
  ${If} ${FileExists} "$INSTDIR\${EXE}"
    MessageBox MB_OK|MB_ICONSTOP "${NAME} is still running. Close it, then run this installer again." /SD IDOK
    Abort
  ${EndIf}
FunctionEnd

Section "Install"
  ; Here rather than in .onInit, so cancelling the directory page leaves the MSI install alone.
  Call RemoveMsi
  Call ReleaseExe
  SetOutPath "$INSTDIR"
  File "/oname=${EXE}" "${SRC}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  WriteRegStr HKCU "${PRODKEY}" "" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" '"$INSTDIR\${EXE}"'
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "screentranslate"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" '"$INSTDIR"'
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "MainBinaryName" "${EXE}"
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  SectionGetSize 0 $0
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" $0

  CreateShortcut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\${EXE}"
  ; Like Tauri: a desktop shortcut only for passive or silent installs that are not updates,
  ; and always when migrating from the MSI (its removal deleted the MSI's desktop shortcut).
  ${If} $RemovedMsi = 1
    CreateShortcut "$DESKTOP\${NAME}.lnk" "$INSTDIR\${EXE}"
  ${ElseIf} $Update = 0
    ${If} $Passive = 1
    ${OrIf} ${Silent}
      CreateShortcut "$DESKTOP\${NAME}.lnk" "$INSTDIR\${EXE}"
    ${EndIf}
  ${EndIf}
SectionEnd

Function .onInstSuccess
  ${If} $Relaunch = 1
    ${If} $Passive = 1
    ${OrIf} ${Silent}
      Exec '"$INSTDIR\${EXE}" $Args'
    ${EndIf}
  ${EndIf}
FunctionEnd

Function un.onInit
  StrCpy $Update 0
  ${un.GetParameters} $R0
  ClearErrors
  ${un.GetOptions} $R0 "/UPDATE" $R1
  ${IfNot} ${Errors}
    StrCpy $Update 1
  ${EndIf}
  ClearErrors
FunctionEnd

Section "Uninstall"
  ; Like Tauri's CheckIfAppIsRunning: offer to close the app first (the installer handles it in update mode).
  ${If} $Update = 0
    nsExec::Exec 'cmd /c tasklist /FI "IMAGENAME eq ${EXE}" /NH | find /I "${EXE}"'
    Pop $R0
    ${If} $R0 = 0
      MessageBox MB_YESNO|MB_ICONEXCLAMATION "${NAME} is running. Close it and continue uninstalling?" /SD IDYES IDYES +2
      Abort
      nsExec::Exec 'taskkill /F /IM ${EXE}'
      Pop $R0
      Sleep 1000
    ${EndIf}
  ${EndIf}
  Delete "$INSTDIR\${EXE}"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$SMPROGRAMS\${NAME}.lnk"
  Delete "$DESKTOP\${NAME}.lnk"
  DeleteRegKey HKCU "${PRODKEY}"
  DeleteRegKey /ifempty HKCU "Software\screentranslate"
  DeleteRegKey HKCU "${UNINSTKEY}"
  ${If} $Update = 0
    DeleteRegValue HKCU "${RUNKEY}" "${NAME}"
  ${EndIf}
  ; Settings and models live in %APPDATA%\com.screentranslate.app and are kept.
  RMDir "$INSTDIR"
SectionEnd
