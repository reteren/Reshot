; Reshot installer (Inno Setup 6).
;
; Not meant to be compiled by hand, build/build-release.ps1 passes the version and the
; staged payload in:
;   ISCC.exe installer.iss /DAppVersion=1.0.0 /DPayloadDir=..\dist\reshot /DOutputDir=..\dist
;
; The payload is the published, self-contained app, reshot-tauri.exe (the settings window),
; and the pinned GPL ffmpeg.exe. All three must stay next to reshot.exe.

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#ifndef PayloadDir
  #define PayloadDir "..\dist\reshot"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif
; The wizard's info page renders plain text, so it gets the generated .txt rendering of
; THIRD-PARTY-NOTICES.md rather than the Markdown source, which would show raw syntax.
#ifndef NoticeFile
  #define NoticeFile "..\dist\THIRD-PARTY-NOTICES.txt"
#endif

#define AppName      "Reshot"
#define AppPublisher "Reshot contributors"
#define AppUrl       "https://github.com/reteren/reshot"
#define AppExe       "reshot.exe"

[Setup]
AppId={{8B4C4E31-2C4F-4E2E-9C1A-6E0B2E9A7D10}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#AppVersion}

; Per-user install: no admin prompt, and the autostart entry Reshot writes lives in
; HKCU anyway, so an elevated install would only cause mismatched permissions.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
; Keep the wizard short: the confirmation step adds a click and tells the user nothing
; they did not just choose themselves.
DisableReadyPage=yes
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName} {#AppVersion}

; Windows 10 2004 (build 19041) is the floor: Windows.Graphics.Capture needs it.
MinVersion=10.0.19041
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

OutputDir={#OutputDir}
OutputBaseFilename=reshot-{#AppVersion}-setup
SetupIconFile=..\src\Reshot.App\Assets\reshot.ico
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
LicenseFile=..\LICENSE
InfoBeforeFile={#NoticeFile}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "Start Reshot when Windows starts"; GroupDescription: "Startup:"
Name: "freeprintscreen"; Description: "Let Reshot use the Print Screen key (turns off the Windows snipping shortcut)"; GroupDescription: "Print Screen key:"

[Files]
; Keep this explicit so the GPL binary is installed beside reshot.exe and tracked for uninstall.
Source: "{#PayloadDir}\ffmpeg.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Excludes: "ffmpeg.exe"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Reshot manages this key itself from its settings; seeding it here just honours the
; checkbox above. uninsdeletevalue keeps an uninstall from leaving a dead autostart.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; \
    ValueName: "reshot"; ValueData: """{app}\{#AppExe}"""; Flags: uninsdeletevalue; Tasks: autostart

; Windows 11 binds Print Screen to the snipping overlay, and that binding is ahead of the
; hotkey Reshot registers, so on a default install the app looks dead on its own default
; hotkey. This is the per-user switch behind Settings > Accessibility > Keyboard > "Use the
; Print screen key to open screen capture"; zero hands the key back to ordinary hotkeys.
; Behind a ticked-by-default task rather than done silently: it is a Windows setting, and
; changing one without saying so is what unwelcome software does. uninsdeletevalue puts the
; Windows default back when Reshot is uninstalled.
Root: HKCU; Subkey: "Control Panel\Keyboard"; ValueType: dword; \
    ValueName: "PrintScreenKeyForSnippingEnabled"; ValueData: 0; \
    Flags: uninsdeletevalue; Tasks: freeprintscreen

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; The app lives in the tray; kill it so its files are not locked during uninstall.
; RunOnceId keeps these from running again on a repeated/partial uninstall.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM reshot.exe"; \
    RunOnceId: "KillReshot"; Flags: runhidden skipifdoesntexist
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM reshot-tauri.exe"; \
    RunOnceId: "KillReshotSettings"; Flags: runhidden skipifdoesntexist

[Code]
// Settings and logs live in %AppData%\reshot. Offer to remove them, but never assume:
// people reinstall far more often than they truly leave.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    DataDir := ExpandConstant('{userappdata}\reshot');
    if DirExists(DataDir) then
      if MsgBox('Remove Reshot settings and logs as well?' + #13#10 + DataDir,
                mbConfirmation, MB_YESNO) = IDYES then
        DelTree(DataDir, True, True, True);
  end;
end;
