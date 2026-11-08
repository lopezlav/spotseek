; Inno Setup script for the Spotseek Windows installer.
; Build with ISCC (preinstalled on GitHub Actions windows-latest runners):
;   ISCC.exe /DAppVersion=1.2.3 packaging\windows\spotseek.iss
; Expected inputs, relative to the repository root:
;   build\windows\Spotseek.exe          - the app window (Rune UI)
;   build\windows\spotseek-backend.exe  - the engine (Rust) that the window runs
;   build\bin\*                 - bundled tools (yt-dlp, ffmpeg, deno)
; Output: build\installer\Spotseek-Setup-windows-x64.exe (one name for every version, so
; https://github.com/lopezlav/spotseek/releases/latest/download/Spotseek-Setup-windows-x64.exe works).
;
; The app updates itself by running this installer with /SILENT /CLOSEAPPLICATIONS /RELAUNCH=1:
; it closes Spotseek, replaces it and starts it again.

#ifndef AppVersion
#define AppVersion "0.0.0"
#endif

; Paths are relative to this script's folder, so go up to the repo root.
#ifndef SourceRoot
#define SourceRoot "..\..\"
#endif

#define MyAppName "Spotseek"
#define MyAppExeName "Spotseek.exe"

[Setup]
AppId={{7ED7C08D-A14C-46A5-8444-9B8FFC28D6F0}
AppName={#MyAppName}
AppVersion={#AppVersion}
AppPublisher=Spotseek
DefaultDirName={autopf}\{#MyAppName}
DisableProgramGroupPage=yes
OutputDir={#SourceRoot}build\installer
OutputBaseFilename=Spotseek-Setup-windows-x64
CloseApplications=yes
RestartApplications=no
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
ArchitecturesAllowed=x64compatible
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
UninstallDisplayName={#MyAppName}
ChangesAssociations=no

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional icons:"

[Files]
Source: "{#SourceRoot}build\windows\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}build\windows\spotseek-backend.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}build\bin\*"; DestDir: "{app}\bin"; Flags: ignoreversion recursesubdirs createallsubdirs; Excludes: "*.pdb"
Source: "{#SourceRoot}build\notices\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Start {#MyAppName}"; Flags: nowait postinstall skipifsilent
; An update from the app (/RELAUNCH=1): start the new version when it is in place.
Filename: "{app}\{#MyAppExeName}"; Flags: nowait runasoriginaluser; Check: Relaunch

[Code]
function Relaunch: Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;
