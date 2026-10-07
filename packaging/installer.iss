#define AppVersion "0.1.0"
[Setup]
AppId={{49DBF80E-5227-44E8-B78F-206A578F593B}
AppName=PhotoSort
AppVersion={#AppVersion}
AppPublisher=domovoyproj
AppPublisherURL=https://github.com/domovoyproj/PhotoSort
DefaultDirName={localappdata}\Programs\PhotoSort
DefaultGroupName=PhotoSort
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=PhotoSort-Setup-{#AppVersion}-windows-x64
SetupIconFile=photosort.ico
UninstallDisplayIcon={app}\PhotoSort.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
[Files]
Source: "..\dist\PhotoSort\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{group}\PhotoSort"; Filename: "{app}\PhotoSort.exe"
Name: "{autodesktop}\PhotoSort"; Filename: "{app}\PhotoSort.exe"; Tasks: desktopicon
[Tasks]
Name: "desktopicon"; Description: "Создать ярлык на рабочем столе"; Flags: unchecked
[Run]
Filename: "{app}\PhotoSort.exe"; Description: "Запустить PhotoSort"; Flags: nowait postinstall skipifsilent
