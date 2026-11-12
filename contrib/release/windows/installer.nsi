; Windows installer definition, compiled by makensis (NSIS) from nix/windows-installer.nix.
; Values come in through -D defines:
;   PRODUCT_NAME, EXE_NAME, VERSION, VI_VERSION, ICON, STAGE_DIR, OUT_FILE
; A fresh install asks nothing: Windows prompts for elevation, the files are
; copied, the window closes and the application is started as the logged-in
; user. A previous version is uninstalled first and a newer one is never
; downgraded. The uninstaller removes the install directory, the Start Menu
; shortcut and the "Apps & features" registry key; the wallet data directory is
; only removed if the user explicitly asks for it (never in silent mode).

Unicode true
SetCompressor /SOLID lzma
SetDatablockOptimize on
CRCCheck on
AllowSkipFiles off
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
!define DATA_DIR_NAME "Liana"

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
    ClearErrors
    ExecWait '"$R0\${UNINSTALLER}" /S _?=$R0' $R1
    ${If} ${Errors}
    ${OrIf} $R1 != 0
      MessageBox MB_ICONSTOP "The previous version of ${PRODUCT_NAME} could not be removed. Close ${PRODUCT_NAME} and run the installer again." /SD IDOK
      Abort
    ${EndIf}
    Delete "$R0\${UNINSTALLER}"
    RMDir "$R0"
  ${EndIf}

  SetOutPath "$INSTDIR"
  ClearErrors
  File "/oname=${EXE_NAME}" "${STAGE_DIR}/${EXE_NAME}"
  File "/oname=LICENCE" "${STAGE_DIR}/LICENCE"
  File "/oname=README.md" "${STAGE_DIR}/README.md"
  ${If} ${Errors}
    MessageBox MB_ICONSTOP "${PRODUCT_NAME} could not be installed. Close ${PRODUCT_NAME} and run the installer again." /SD IDOK
    Abort
  ${EndIf}
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

  ClearErrors
  Delete "$INSTDIR\${EXE_NAME}"
  ${If} ${Errors}
    MessageBox MB_ICONSTOP "${PRODUCT_NAME} is running. Close it and try again." /SD IDOK
    Abort
  ${EndIf}
  Delete "$INSTDIR\LICENCE"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\${UNINSTALLER}"
  RMDir "$INSTDIR"

  Delete "$SMPROGRAMS\${PRODUCT_NAME}.lnk"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"

  SetShellVarContext current
  ${If} ${FileExists} "$APPDATA\${DATA_DIR_NAME}\*.*"
    MessageBox MB_YESNO|MB_DEFBUTTON2|MB_ICONEXCLAMATION \
      "Also delete the wallet data in $APPDATA\${DATA_DIR_NAME}?$\r$\n$\r$\nThis folder holds the wallets, settings and daemon data of both Liana and Liana Business. Choose No to keep it." \
      /SD IDNO IDYES delete_data
    Goto keep_data
    delete_data:
    RMDir /r "$APPDATA\${DATA_DIR_NAME}"
    keep_data:
  ${EndIf}
SectionEnd
