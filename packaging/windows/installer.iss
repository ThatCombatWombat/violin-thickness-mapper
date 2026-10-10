#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

#define AppName "Violin Thickness Mapper"
#define AppExe "violin-thickness-mapper.exe"

[Setup]
AppId={{7E2A8ED1-ACBB-47F6-A0FA-2F881574F17D}
AppName={#AppName}
AppVersion={#AppVersion}
DefaultDirName={localappdata}\Programs\Violin Thickness Mapper
DefaultGroupName={#AppName}
PrivilegesRequired=lowest
OutputDir=..\..\dist
OutputBaseFilename=ViolinThicknessMapper-windows-x86_64-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\{#AppExe}

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "..\..\dist\stage\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\stage\violin_back.jpg"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\stage\violin_front.jpg"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\dist\stage\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExe}"; WorkingDir: "{app}"; Description: "Launch {#AppName}"; Flags: postinstall nowait skipifsilent
