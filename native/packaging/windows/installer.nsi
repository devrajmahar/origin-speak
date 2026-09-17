Unicode true
RequestExecutionLevel user
SetCompressor /SOLID lzma

!ifndef APP_VERSION
  !error "APP_VERSION must be supplied by package.ps1"
!endif
!ifndef BUILD_EXE
  !error "BUILD_EXE must be supplied by package.ps1"
!endif
!ifndef ICON_FILE
  !error "ICON_FILE must be supplied by package.ps1"
!endif
!ifndef OUTPUT_FILE
  !error "OUTPUT_FILE must be supplied by package.ps1"
!endif

!include "MUI2.nsh"

Name "ListenOS"
OutFile "${OUTPUT_FILE}"
InstallDir "$LOCALAPPDATA\Programs\ListenOS"
InstallDirRegKey HKCU "Software\ListenOS" "InstallDir"

!define MUI_ABORTWARNING
!define MUI_ICON "${ICON_FILE}"
!define MUI_UNICON "${ICON_FILE}"
!define MUI_FINISHPAGE_RUN "$INSTDIR\ListenOS.exe"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "ListenOS" SEC_LISTENOS
  SectionIn RO
  SetShellVarContext current
  SetOutPath "$INSTDIR"

  File /oname=ListenOS.exe "${BUILD_EXE}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  WriteRegStr HKCU "Software\ListenOS" "InstallDir" "$INSTDIR"

  ; listenos:// is activation-only. The native app intentionally ignores the
  ; payload and forwards second launches to the already-running dashboard.
  WriteRegStr HKCU "Software\Classes\listenos" "" "URL:ListenOS Protocol"
  WriteRegStr HKCU "Software\Classes\listenos" "URL Protocol" ""
  WriteRegStr HKCU "Software\Classes\listenos\DefaultIcon" "" "$INSTDIR\ListenOS.exe,0"
  WriteRegStr HKCU "Software\Classes\listenos\shell\open\command" "" '"$INSTDIR\ListenOS.exe" "%1"'

  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS" "DisplayName" "ListenOS"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS" "Publisher" "ListenOS"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS" "DisplayIcon" "$INSTDIR\ListenOS.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\ListenOS"
  CreateShortcut "$SMPROGRAMS\ListenOS\ListenOS.lnk" "$INSTDIR\ListenOS.exe"
  CreateShortcut "$SMPROGRAMS\ListenOS\Uninstall ListenOS.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section /o "Desktop shortcut" SEC_DESKTOP
  SetShellVarContext current
  CreateShortcut "$DESKTOP\ListenOS.lnk" "$INSTDIR\ListenOS.exe"
SectionEnd

Section "Uninstall"
  SetShellVarContext current

  ; Native autostart registers this value. Remove it during uninstall so the
  ; login shell never points at a deleted executable.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "ListenOS"
  DeleteRegKey HKCU "Software\Classes\listenos"

  Delete "$DESKTOP\ListenOS.lnk"
  Delete "$SMPROGRAMS\ListenOS\ListenOS.lnk"
  Delete "$SMPROGRAMS\ListenOS\Uninstall ListenOS.lnk"
  RMDir "$SMPROGRAMS\ListenOS"

  Delete "$INSTDIR\ListenOS.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\ListenOS"
  DeleteRegKey HKCU "Software\ListenOS"
SectionEnd
