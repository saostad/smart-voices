; Inno Setup script for the Windows installer.
; Build: iscc /DAppVersion=2.0.0 packaging\windows\smart-voices.iss
; Expects target\release\smart-voices.exe; writes
; target\installer\smart-voices-windows-x86_64-setup.exe.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{BE50E306-D4C2-479E-8605-EDFB66CF0773}
AppName=Smart Voices
AppVersion={#AppVersion}
AppPublisher=Smart Voices
AppPublisherURL=https://github.com/saostad/smart-voices
DefaultDirName={autopf}\Smart Voices
DefaultGroupName=Smart Voices
DisableProgramGroupPage=yes
; Per-user install by default (no admin prompt); the dialog lets the user
; install for all users instead.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=smart-voices.ico
UninstallDisplayIcon={app}\smart-voices.exe
UninstallDisplayName=Smart Voices
OutputDir=..\..\target\installer
OutputBaseFilename=smart-voices-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=force

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\target\release\smart-voices.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\target\release\onnxruntime.dll"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Smart Voices"; Filename: "{app}\smart-voices.exe"
Name: "{autodesktop}\Smart Voices"; Filename: "{app}\smart-voices.exe"; Tasks: desktopicon

[Registry]
; Remove the "Launch at Login" value the app writes, if any.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Smart Voices"; Flags: uninsdeletevalue dontcreatekey

[Run]
Filename: "{app}\smart-voices.exe"; Description: "{cm:LaunchProgram,Smart Voices}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM smart-voices.exe"; Flags: runhidden; RunOnceId: "StopSmartVoices"
