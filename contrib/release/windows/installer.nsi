; Windows installer definition, compiled by makensis (NSIS) from nix/windows-installer.nix.
; Values come in through -D defines:
;   PRODUCT_NAME, EXE_NAME, VERSION, VI_VERSION, ICON, STAGE_DIR, OUT_FILE
; A fresh install asks nothing: Windows prompts for elevation, the files are
; copied, the window closes and the application is started as the logged-in
; user. A previous version is uninstalled first and a newer one is never
; downgraded. The uninstaller never touches anything outside the install
; directory, the Start Menu shortcut and the "Apps & features" registry key.

Unicode true
SetCompressor /SOLID lzma
SetDatablockOptimize on
CRCCheck on
RequestExecutionLevel admin
ManifestSupportedOS all
ManifestDPIAware true

!include "MUI2.nsh"
!include "x64.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "WordFunc.nsh"

!define PUBLISHER "Wizardsardine"
!define URL "https://wizardsardine.com/liana/"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT_NAME}"
!define UNINSTALLER "uninstall.exe"

Name "${PRODUCT_NAME}"
OutFile "${OUT_FILE}"
InstallDir "$PROGRAMFILES64\${PRODUCT_NAME}"
BrandingText "${PRODUCT_NAME} ${VERSION}"
ShowInstDetails nevershow
ShowUninstDetails nevershow
AutoCloseWindow true

VIProductVersion "${VI_VERSION}"
VIFileVersion "${VI_VERSION}"
VIAddVersionKey "ProductName" "${PRODUCT_NAME}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${PRODUCT_NAME} Setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "${PUBLISHER}"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "${PRODUCT_NAME} requires a 64-bit version of Windows." /SD IDOK
    Abort
  ${EndIf}
  SetRegView 64

  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "DisplayVersion"
  ${If} $0 != ""
    ${VersionCompare} "${VERSION}" "$0" $1
    ${If} $1 == 2
      MessageBox MB_ICONSTOP "A newer version of ${PRODUCT_NAME} ($0) is already installed." /SD IDOK
      Abort
    ${EndIf}
  ${EndIf}

  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${EndIf}
FunctionEnd

Section "-Install"
  SetShellVarContext all
  SetRegView 64

  ReadRegStr $R0 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $R0 != ""
  ${AndIf} ${FileExists} "$R0\${UNINSTALLER}"
    ExecWait '"$R0\${UNINSTALLER}" /S _?=$R0'
    Delete "$R0\${UNINSTALLER}"
    RMDir "$R0"
  ${EndIf}

  SetOutPath "$INSTDIR"
  File "/oname=${EXE_NAME}" "${STAGE_DIR}/${EXE_NAME}"
  File "/oname=LICENCE" "${STAGE_DIR}/LICENCE"
  File "/oname=README.md" "${STAGE_DIR}/README.md"
  WriteUninstaller "$INSTDIR\${UNINSTALLER}"

  CreateShortCut "$SMPROGRAMS\${PRODUCT_NAME}.lnk" "$INSTDIR\${EXE_NAME}" "" "$INSTDIR\${EXE_NAME}" 0

  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${EXE_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "URLInfoAbout" "${URL}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\${UNINSTALLER}"'
  WriteRegStr HKLM "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINSTALLER}" /S'
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Function .onInstSuccess
  ${IfNot} ${Silent}
    Exec '"$WINDIR\explorer.exe" "$INSTDIR\${EXE_NAME}"'
  ${EndIf}
FunctionEnd

Function un.onInit
  SetRegView 64
FunctionEnd

Section "Uninstall"
  SetShellVarContext all

  Delete "$INSTDIR\${EXE_NAME}"
  Delete "$INSTDIR\LICENCE"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\${UNINSTALLER}"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"
SectionEnd
