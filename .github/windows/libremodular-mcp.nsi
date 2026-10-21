; Installeur par utilisateur de LibreModular-MCP : aucun droit d'administrateur.
; Construction : makensis -DVERSION=0.1.0 -DPAYLOAD=<dossier> -DOUTFILE=<installeur.exe> [-DICON=<logo.ico>] libremodular-mcp.nsi
Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "StrFunc.nsh"
${StrStr}
${UnStrStr}
${UnStrRep}

!ifndef VERSION
  !error "définir -DVERSION=x.y.z"
!endif
!ifndef PAYLOAD
  !error "définir -DPAYLOAD=<dossier>"
!endif
!ifndef OUTFILE
  !define OUTFILE "libremodular-mcp-${VERSION}-setup.exe"
!endif

!define NAME "LibreModular-MCP"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${NAME}"

!ifdef ICON
  !define MUI_ICON "${ICON}"
  !define MUI_UNICON "${ICON}"
!endif

Name "${NAME} ${VERSION}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\${NAME}"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
SetCompressor /SOLID lzma
BrandingText "${NAME} ${VERSION}"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${NAME}"
VIAddVersionKey "FileDescription" "${NAME} installer"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "MIT"

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"
  File "${PAYLOAD}\libremodular-mcp.exe"
  File "${PAYLOAD}\libremodular-launch.exe"
  File "${PAYLOAD}\LICENSE.txt"
  SetOutPath "$INSTDIR\bridges"
  File /r "${PAYLOAD}\bridges\*.py"
  SetOutPath "$INSTDIR\docs"
  File /r "${PAYLOAD}\docs\*.*"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ReadRegStr $0 HKCU "Environment" "Path"
  ${StrStr} $1 ";$0;" ";$INSTDIR;"
  ${If} $1 == ""
    ${If} $0 == ""
      WriteRegExpandStr HKCU "Environment" "Path" "$INSTDIR"
    ${Else}
      WriteRegExpandStr HKCU "Environment" "Path" "$0;$INSTDIR"
    ${EndIf}
    SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
  ${EndIf}

  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  ReadRegStr $0 HKCU "Environment" "Path"
  ${UnStrStr} $1 ";$0;" ";$INSTDIR;"
  ${If} $1 != ""
    ${UnStrRep} $2 ";$0;" ";$INSTDIR;" ";"
    StrCpy $2 $2 -1
    StrCpy $2 $2 "" 1
    WriteRegExpandStr HKCU "Environment" "Path" "$2"
    SendMessage ${HWND_BROADCAST} ${WM_WININICHANGE} 0 "STR:Environment" /TIMEOUT=5000
  ${EndIf}
  Delete "$INSTDIR\libremodular-mcp.exe"
  Delete "$INSTDIR\libremodular-launch.exe"
  Delete "$INSTDIR\LICENSE.txt"
  RMDir /r "$INSTDIR\bridges"
  RMDir /r "$INSTDIR\docs"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
SectionEnd
