; YumeDock Inno Setup installer script.
;
; Build (locally or in CI):
;   iscc /DAPP_VERSION=0.1.1 /DAPP_SOURCE=..\target\release\YumeDock.exe yumedock.iss
;
; Produces installer/Output/YumeDock-Setup.exe.
;
; Design notes:
;  - Per-user install ({localappdata}\Programs\YumeDock) so no UAC/admin prompt.
;    A taskbar-replacement app must not require elevation.
;  - User config (%LOCALAPPDATA%\YumeDock\config.json) is NEVER touched: it
;    lives outside the install dir and survives uninstall/reinstall.

#ifndef APP_VERSION
  #define APP_VERSION "0.1.0.0"
#endif

#ifndef APP_SOURCE
  #define APP_SOURCE "..\target\release\YumeDock.exe"
#endif

[Setup]
AppId={{8F2C4A1E-3D5B-4E6F-9A2C-7B8D1E5F0A23}
AppName=YumeDock
AppVersion={#APP_VERSION}
AppVerName=YumeDock {#APP_VERSION}
AppPublisher=YumeDock
AppPublisherURL=https://github.com/sanketpatel32/YumeDock
AppSupportURL=https://github.com/sanketpatel32/YumeDock/issues
AppUpdatesURL=https://github.com/sanketpatel32/YumeDock/releases
AppContact=https://github.com/sanketpatel32/YumeDock
AppReadmeFile=https://github.com/sanketpatel32/YumeDock#readme
; Per-user install: no admin, no UAC prompt.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={localappdata}\Programs\YumeDock
DefaultGroupName=YumeDock
DisableProgramGroupPage=yes
DisableDirPage=yes
; Uninstaller entry in Add/Remove Programs.
CreateUninstallRegKey=yes
UninstallDisplayIcon={app}\YumeDock.exe
UninstallDisplayName=YumeDock
; Single self-contained exe installer.
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
OutputDir=Output
OutputBaseFilename=YumeDock-Setup
; Do not close the installer window abruptly on the final page.
CloseApplications=force
RestartApplications=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
; The compiled release exe (source path injected via /DAPP_SOURCE).
Source: "{#APP_SOURCE}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Start Menu shortcut.
Name: "{group}\YumeDock"; Filename: "{app}\YumeDock.exe"; Comment: "Lightweight macOS-style dock and top bar for Windows 11"
Name: "{group}\YumeDock (safe mode)"; Filename: "{app}\YumeDock.exe"; Parameters: "--safe-mode"; Comment: "Run without hiding the Windows taskbar"
Name: "{group}\Visit YumeDock on GitHub"; Filename: "https://github.com/sanketpatel32/YumeDock"
; Uninstall shortcut in the Start Menu group for discoverability.
Name: "{group}\Uninstall YumeDock"; Filename: "{uninstallexe}"; Comment: "Remove YumeDock (keeps your settings)"
; Optional desktop shortcut (off by default).
Name: "{autodesktop}\YumeDock"; Filename: "{app}\YumeDock.exe"; Tasks: desktopicon; Comment: "Lightweight macOS-style dock and top bar for Windows 11"

[Run]
; Offer to launch after install (only in interactive installs).
Filename: "{app}\YumeDock.exe"; Description: "Launch YumeDock now"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Gracefully stop a running instance before uninstalling so the taskbar is
; restored by the app's own shutdown path.
Filename: "{cmd}"; Parameters: "/C taskkill /IM YumeDock.exe /F 2>nul"; Flags: runhidden; RunOnceId: "StopYumeDock"

[UninstallDelete]
; Remove the install dir contents, but NEVER the user config dir
; (%LOCALAPPDATA%\YumeDock), which is separate and holds config.json.
Type: filesandordirs; Name: "{app}"

[Code]
function InitializeSetup(): Boolean;
begin
  Result := True;
end;
