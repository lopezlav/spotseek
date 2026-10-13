; Inno Setup script for the Spotseek Windows installer.
; Build with ISCC (preinstalled on GitHub Actions windows-latest runners):
;   ISCC.exe /DAppVersion=1.2.3 packaging\windows\spotseek.iss
; Expected inputs, relative to the repository root:
;   build\windows\Spotseek.exe  - the packaged app executable
;   build\bin\*                 - bundled tools (sockseek, yt-dlp, ffmpeg, ffprobe)
; Output: build\installer\Spotseek-Setup-<version>.exe

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
OutputBaseFilename=Spotseek-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequiredOverridesAllowed=dialog
UninstallDisplayName={#MyAppName}
ChangesAssociations=no

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional icons:"

[Files]
Source: "{#SourceRoot}build\windows\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceRoot}build\bin\*"; DestDir: "{app}\bin"; Flags: ignoreversion recursesubdirs createallsubdirs; Excludes: "*.pdb"

[Icons]
Name: "{autoprograms}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Start {#MyAppName}"; Flags: nowait postinstall skipifsilent
