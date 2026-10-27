; The Windows installer, built by CI with Inno Setup 6:
;   iscc /DAppVersion=v0.1.0 packaging\windows\screenpen.iss
; Installs for the current user only (no administrator prompt) into
; %LOCALAPPDATA%\Programs\screenpen, with a Start menu entry and an uninstaller.

#ifndef AppVersion
  #define AppVersion "dev"
#endif

[Setup]
AppId={{D70F1508-C710-4FEA-BCCB-B8C56F7E0109}
AppName=screenpen
AppVersion={#AppVersion}
AppVerName=screenpen {#AppVersion}
AppPublisher=Ivan Khramov
AppPublisherURL=https://github.com/IvanKhramov/screenpen
AppSupportURL=https://github.com/IvanKhramov/screenpen/issues
AppUpdatesURL=https://github.com/IvanKhramov/screenpen/releases
DefaultDirName={autopf}\screenpen
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\..\assets\screenpen.ico
UninstallDisplayIcon={app}\screenpen.exe
UninstallDisplayName=screenpen
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
OutputDir=..\..\target\installer
OutputBaseFilename=screenpen-setup
; screenpen is stopped with "--quit" below; Restart Manager can't close a tray app.
CloseApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "ru"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
en.Autostart=Start screenpen when I sign in to Windows
ru.Autostart=Запускать screenpen при входе в Windows

[Tasks]
Name: "autostart"; Description: "{cm:Autostart}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; Flags: unchecked

[Files]
Source: "..\..\target\release\screenpen.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\screenpen"; Filename: "{app}\screenpen.exe"
Name: "{autodesktop}\screenpen"; Filename: "{app}\screenpen.exe"; Tasks: desktopicon

[Registry]
; The same value the tray menu's "Start with Windows" writes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; \
  ValueName: "screenpen"; ValueData: """{app}\screenpen.exe"""; Tasks: autostart
; Removed on uninstall even when it was turned on later from the tray.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; \
  ValueName: "screenpen"; Flags: uninsdeletevalue dontcreatekey

[Run]
Filename: "{app}\screenpen.exe"; Description: "{cm:LaunchProgram,screenpen}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{app}\screenpen.exe"; Parameters: "--quit"; Flags: runhidden waituntilterminated; RunOnceId: "QuitScreenpen"

[Code]
// A running copy (installed or portable) holds the exe: stop it first, using the
// new exe so this works on the first install too.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  ExtractTemporaryFile('screenpen.exe');
  Exec(ExpandConstant('{tmp}\screenpen.exe'), '--quit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;
