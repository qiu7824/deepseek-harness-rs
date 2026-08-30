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
#define Variant "core"
#endif
#ifndef IconFile
#define IconFile "deepseek-black.ico"
#endif
#ifndef ChineseMessages
#define ChineseMessages "ChineseSimplified.isl"
#endif
#if Variant == "core"
#define MyAppId "{{A6F42843-79DD-4FA1-91D2-0B71F8974B78}"
#else
#define MyAppId "{{7D47BC56-AB4A-4E87-8E62-652A319F6C4F}"
#endif
[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=DeepSeek Harness-rs
DefaultDirName={localappdata}\Programs\DeepSeek Harness-rs\{#Variant}
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
UninstallDisplayIcon={app}\dsh-launcher.exe
ShowLanguageDialog=auto
LanguageDetectionMethod=uilanguage
[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "chinesesimp"; MessagesFile: "{#ChineseMessages}"
[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional tasks:"; Flags: unchecked; Languages: english
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "附加任务："; Flags: unchecked; Languages: chinesesimp
[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{group}\DeepSeek Harness-rs Launcher"; Filename: "{app}\dsh-launcher.exe"; WorkingDir: "{app}"; Languages: english
Name: "{group}\DeepSeek Harness-rs 启动器"; Filename: "{app}\dsh-launcher.exe"; WorkingDir: "{app}"; Languages: chinesesimp
Name: "{autodesktop}\DeepSeek Harness-rs"; Filename: "{app}\dsh-launcher.exe"; WorkingDir: "{app}"; Tasks: desktopicon
Name: "{group}\Uninstall DeepSeek Harness-rs"; Filename: "{uninstallexe}"; Languages: english
Name: "{group}\卸载 DeepSeek Harness-rs"; Filename: "{uninstallexe}"; Languages: chinesesimp
[Run]
Filename: "{app}\dsh-launcher.exe"; Description: "Launch DeepSeek Harness-rs"; Flags: postinstall nowait skipifsilent; Languages: english
Filename: "{app}\dsh-launcher.exe"; Description: "启动 DeepSeek Harness-rs"; Flags: postinstall nowait skipifsilent; Languages: chinesesimp
