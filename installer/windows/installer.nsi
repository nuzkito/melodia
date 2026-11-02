; Melodia installer for Windows (NSIS 3 + Modern UI 2).
;
; docker/build-windows.sh builds it with:
;   makensis -DVERSION=0.1.0 -DDIST=… -DICON=… -DOUTFILE=… installer.nsi
; (DIST is the folder with melodia.exe and WebView2Loader.dll).
;
; It installs only for the current user, so it needs no administrator rights:
; %LOCALAPPDATA%\Programs\Melodia.
;
; The uninstaller (uninstall.exe) opens from Settings → Apps or from Start → Melodia → Uninstall
; Melodia. With /S, it uninstalls without a question and keeps the settings and the Spotify
; session.

Unicode true
ManifestDPIAware true
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "LogicLib.nsh"

!define APP_NAME "Melodia"
!define APP_EXE "melodia.exe"
; The helper process of «Play in this app» in earlier builds (now it runs in the app). The
; installer still closes and removes it when it updates an older installation.
!define WEB_EXE "melodia-web.exe"
!define APP_KEY "Software\Melodia"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Melodia"
; Start Menu folder with the app and its uninstaller.
!define SM_FOLDER "$SMPROGRAMS\${APP_NAME}"
!define SM_UNINSTALL "Uninstall ${APP_NAME}.lnk"

Name "${APP_NAME}"
OutFile "${OUTFILE}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\${APP_NAME}"
InstallDirRegKey HKCU "${APP_KEY}" "InstallDir"
BrandingText "${APP_NAME} ${VERSION}"

!define MUI_ICON "${ICON}"
!define MUI_UNICON "${ICON}"
!define MUI_ABORTWARNING
!define MUI_UNABORTWARNING
!define MUI_COMPONENTSPAGE_NODESC
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Open ${APP_NAME}"

; Installer pages.
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!define MUI_PAGE_CUSTOMFUNCTION_LEAVE CheckInstDir
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

; Uninstaller pages.
!define MUI_WELCOMEPAGE_TEXT "This wizard will uninstall ${APP_NAME} from your computer.$\r$\n$\r$\nIf ${APP_NAME} is open, you will be asked to close it before continuing.$\r$\n$\r$\nClick Next to continue."
!insertmacro MUI_UNPAGE_WELCOME
!define MUI_COMPONENTSPAGE_TEXT_TOP "Choose what to delete. Your settings and Spotify session are kept unless you check the second option."
!define MUI_COMPONENTSPAGE_TEXT_COMPLIST "What to delete:"
!insertmacro MUI_UNPAGE_COMPONENTS
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_UNPAGE_FINISH

!insertmacro MUI_LANGUAGE "English"

VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileDescription" "${APP_NAME} installer"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductVersion" "${VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "LegalCopyright" "MIT"

; ---------------------------------------------------------------------------
; Helpers shared by the installer and the uninstaller (PREFIX is "" or "un.").
; ---------------------------------------------------------------------------

; IsRunning: pushes 1 if melodia.exe (or the old helper) runs for this user, or else 0 (also 0 if
; tasklist is not available).
!macro IS_RUNNING_FUNC PREFIX
Function ${PREFIX}IsRunningExe
    ; In: image name on the stack. Out: 1 / 0 on the stack.
    Exch $0
    Push $1
    Push $2
    ; CSV without header: a match starts with "image.exe". If there is no match, tasklist prints a
    ; (localized) "no tasks" message.
    nsExec::ExecToStack '"$SYSDIR\tasklist.exe" /NH /FO CSV /FI "IMAGENAME eq $0"'
    Pop $1 ; exit code
    Pop $2 ; output
    ${If} $1 != 0
        StrCpy $0 0
    ${Else}
        ; Skip line breaks or spaces at the start.
        ${Do}
            StrCpy $1 $2 1
            ${If} $1 == "$\r"
            ${OrIf} $1 == "$\n"
            ${OrIf} $1 == " "
                StrCpy $2 $2 "" 1
            ${Else}
                ${Break}
            ${EndIf}
        ${Loop}
        StrLen $1 '"$0"'
        StrCpy $2 $2 $1
        ${If} $2 == '"$0"' ; case-insensitive
            StrCpy $0 1
        ${Else}
            StrCpy $0 0
        ${EndIf}
    ${EndIf}
    Pop $2
    Pop $1
    Exch $0
FunctionEnd

Function ${PREFIX}IsRunning
    Push $0
    Push "${APP_EXE}"
    Call ${PREFIX}IsRunningExe
    Pop $0
    ${If} $0 == 0
        Push "${WEB_EXE}"
        Call ${PREFIX}IsRunningExe
        Pop $0
    ${EndIf}
    Exch $0
FunctionEnd

; Closes the app: first normally (WM_CLOSE), then by force after ~5 s.
Function ${PREFIX}CloseApp
    Push $0
    Push $1
    nsExec::Exec '"$SYSDIR\taskkill.exe" /IM "${APP_EXE}" /IM "${WEB_EXE}"'
    Pop $0
    StrCpy $1 0
    ${Do}
        Sleep 500
        Call ${PREFIX}IsRunning
        Pop $0
        ${If} $0 == 0
            ${Break}
        ${EndIf}
        IntOp $1 $1 + 1
        ${If} $1 >= 10
            nsExec::Exec '"$SYSDIR\taskkill.exe" /F /T /IM "${APP_EXE}" /IM "${WEB_EXE}"'
            Pop $0
            Sleep 1000
            ${Break}
        ${EndIf}
    ${Loop}
    ; Give the child processes of WebView2 time to release the cache.
    Sleep 500
    Pop $1
    Pop $0
FunctionEnd

; Makes sure that the app does not run, so that no file stays locked. Asks the user (or, in silent
; mode, closes it). Stops if the user cancels.
Function ${PREFIX}EnsureClosed
    Push $0
    ${Do}
        Call ${PREFIX}IsRunning
        Pop $0
        ${If} $0 == 0
            ${Break}
        ${EndIf}
        ${If} ${Silent}
            Call ${PREFIX}CloseApp
            ${Break}
        ${EndIf}
        MessageBox MB_YESNOCANCEL|MB_ICONEXCLAMATION|MB_DEFBUTTON1 \
            "${APP_NAME} is open and must be closed to continue.$\r$\n$\r$\n\
Yes: close it now (playback will stop).$\r$\n\
No: I will close it myself; check again.$\r$\n\
Cancel: exit without making changes." \
            IDYES close IDNO retry
        Quit
    close:
        Call ${PREFIX}CloseApp
    retry:
    ${Loop}
    Pop $0
FunctionEnd
!macroend

!insertmacro IS_RUNNING_FUNC ""
!insertmacro IS_RUNNING_FUNC "un."

; Deletes the installed files from DIR (only known files, never recursive).
!macro REMOVE_APP_FILES DIR
    Delete "${DIR}\${APP_EXE}"
    Delete "${DIR}\${WEB_EXE}"
    Delete "${DIR}\WebView2Loader.dll"
    Delete "${DIR}\uninstall.exe"
    RMDir "${DIR}"
!macroend

; Removes all shortcuts (current layout and the old single shortcut).
!macro REMOVE_SHORTCUTS
    Delete "${SM_FOLDER}\${APP_NAME}.lnk"
    Delete "${SM_FOLDER}\${SM_UNINSTALL}"
    RMDir "${SM_FOLDER}"
    Delete "$SMPROGRAMS\${APP_NAME}.lnk"
!macroend

; ---------------------------------------------------------------------------
; Installer
; ---------------------------------------------------------------------------

Var PrevInstDir

Function .onInit
    ReadRegStr $PrevInstDir HKCU "${UNINSTALL_KEY}" "InstallLocation"
FunctionEnd

; Refuses incorrect folders (a drive root, for example C:\).
Function CheckInstDir
    Push $0
    StrLen $0 "$INSTDIR"
    ${If} $0 <= 3
        Pop $0
        MessageBox MB_OK|MB_ICONEXCLAMATION "Choose a folder, not the root of a drive."
        Abort
    ${EndIf}
    Pop $0
FunctionEnd

Section "${APP_NAME}" SecApp
    SectionIn RO
    Call EnsureClosed

    ; It was installed before in a different folder: remove those files (only the app's files),
    ; so that nothing stays. The settings and the cache stay.
    ${If} $PrevInstDir != ""
    ${AndIf} $PrevInstDir != $INSTDIR
    ${AndIf} ${FileExists} "$PrevInstDir\${APP_EXE}"
        DetailPrint "Removing the previous installation from $PrevInstDir"
        !insertmacro REMOVE_APP_FILES "$PrevInstDir"
    ${EndIf}

    SetOutPath "$INSTDIR"
    File "${DIST}/${APP_EXE}"
    ; WebView2 loader, for «Play in this app» (the app needs it to start).
    File "${DIST}/WebView2Loader.dll"
    ; Older versions ran the web player in a helper process.
    Delete "$INSTDIR\${WEB_EXE}"
    WriteUninstaller "$INSTDIR\uninstall.exe"

    ; Start Menu: Melodia folder with the app and its uninstaller.
    !insertmacro REMOVE_SHORTCUTS
    Delete "$DESKTOP\${APP_NAME}.lnk" ; recreated if that option is checked
    CreateDirectory "${SM_FOLDER}"
    CreateShortcut "${SM_FOLDER}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
    CreateShortcut "${SM_FOLDER}\${SM_UNINSTALL}" "$INSTDIR\uninstall.exe" \
        "" "$INSTDIR\uninstall.exe" 0 SW_SHOWNORMAL "" "Uninstall ${APP_NAME}"

    ; Entry in Settings > Apps (for each user).
    WriteRegStr HKCU "${APP_KEY}" "InstallDir" "$INSTDIR"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${APP_NAME}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
    ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
    IntFmt $0 "0x%08X" $0
    WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" "$0"
SectionEnd

Section "Desktop shortcut" SecDesktop
    CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
SectionEnd

; ---------------------------------------------------------------------------
; Uninstaller
; ---------------------------------------------------------------------------

Function un.onInit
    ; $INSTDIR is the folder of the uninstaller. Refuse unusual locations (a drive root, or a
    ; folder without the app's uninstaller), so that nothing else changes.
    StrLen $0 "$INSTDIR"
    ${If} $0 <= 3
    ${OrIfNot} ${FileExists} "$INSTDIR\uninstall.exe"
        MessageBox MB_OK|MB_ICONSTOP "${APP_NAME} was not found in $INSTDIR." /SD IDOK
        Abort
    ${EndIf}
FunctionEnd

Section "un.${APP_NAME}" UnSecApp
    SectionIn RO
    Call un.EnsureClosed

    !insertmacro REMOVE_APP_FILES "$INSTDIR"
    !insertmacro REMOVE_SHORTCUTS
    Delete "$DESKTOP\${APP_NAME}.lnk"
    DeleteRegKey HKCU "${UNINSTALL_KEY}"
    DeleteRegKey HKCU "${APP_KEY}"

    ; The cache of «Play in this app» (WebView2 profile, under %LOCALAPPDATA%\Melodia\data). Only
    ; the app's subfolder is removed recursively. The parent is removed only if it becomes empty.
    ${If} $LOCALAPPDATA != ""
        RMDir /r "$LOCALAPPDATA\${APP_NAME}\data"
        RMDir "$LOCALAPPDATA\${APP_NAME}"
    ${EndIf}
SectionEnd

; Not selected by default. A silent uninstall (/S) also keeps them.
Section /o "un.Also delete my settings and Spotify session" UnSecData
    ; %APPDATA%\Melodia: settings.json, session.json and the log.
    ${If} $APPDATA != ""
    ${AndIf} "$APPDATA\${APP_NAME}" != $INSTDIR
        DetailPrint "Deleting settings and session from $APPDATA\${APP_NAME}"
        RMDir /r "$APPDATA\${APP_NAME}"
    ${EndIf}
SectionEnd
