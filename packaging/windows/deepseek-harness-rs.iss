#define MyAppName "DeepSeek Harness-rs"
#ifndef MyAppVersion
#define MyAppVersion "0.1.0-rc.8"
#endif
#ifndef SourceDir
#define SourceDir "dist\deepseek-harness-rs-windows-x86_64"
#endif
#ifndef OutputDir
#define OutputDir "dist"
#endif
#ifndef Variant
#define Variant "full"
#endif
#ifndef IconFile
#define IconFile "deepseek-black.ico"
#endif
#ifndef ChineseMessages
#define ChineseMessages "ChineseSimplified.isl"
#endif
[Setup]
AppId={{A6F42843-79DD-4FA1-91D2-0B71F8974B78}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=DeepSeek Harness-rs
DefaultDirName={localappdata}\Programs\DeepSeek Harness-rs
DefaultGroupName=DeepSeek Harness-rs
OutputDir={#OutputDir}
OutputBaseFilename=deepseek-harness-rs-v{#MyAppVersion}-windows-x86_64-{#Variant}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\deepseek harness-rs.exe
ShowLanguageDialog=no
[Languages]
Name: "chinesesimp"; MessagesFile: "{#ChineseMessages}"
[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked
[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{group}\DeepSeek Harness-rs 运行管理器"; Filename: "{app}\启动DeepSeek Harness-rs.cmd"; WorkingDir: "{app}"
Name: "{autodesktop}\DeepSeek Harness-rs"; Filename: "{app}\启动DeepSeek Harness-rs.cmd"; WorkingDir: "{app}"; Tasks: desktopicon
Name: "{group}\卸载 DeepSeek Harness-rs"; Filename: "{uninstallexe}"
[Run]
Filename: "{app}\启动DeepSeek Harness-rs.cmd"; Description: "打开 DeepSeek Harness-rs"; Flags: postinstall nowait skipifsilent
