#ifndef MyAppVersion
#define MyAppVersion "0.1.3-alpha.3"
#endif
#ifndef SourceDir
#define SourceDir "dist\deepseek-harness-rs-v0.1.3-alpha.3-windows-x86_64-core"
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
#define MyVariantDisplay "Core"
#elif Variant == "skin"
#define MyAppId "{{7D47BC56-AB4A-4E87-8E62-652A319F6C4F}"
#define MyVariantDisplay "Skin"
#elif Variant == "free"
#define MyAppId "{{F0B73461-F37A-407E-BE7D-71D6B84139D2}"
#define MyVariantDisplay "Free"
#else
#error Unknown release variant
#endif
#define MyAppName "DeepSeek Harness-rs (" + MyVariantDisplay + ")"
[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=DeepSeek Harness-rs
DefaultDirName={localappdata}\Programs\DeepSeek Harness-rs\{#Variant}
DefaultGroupName={#MyAppName}
OutputDir={#OutputDir}
OutputBaseFilename=deepseek-harness-rs-v{#MyAppVersion}-windows-x86_64-{#Variant}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
SetupIconFile={#IconFile}
UninstallDisplayIcon={app}\dsh-launcher.exe
ShowLanguageDialog=auto
LanguageDetectionMethod=uilanguage
[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "chinesesimp"; MessagesFile: "{#ChineseMessages}"
[CustomMessages]
english.DesktopShortcut=Create a desktop shortcut
english.AdditionalTasks=Additional tasks:
english.LauncherName=DeepSeek Harness-rs Launcher
english.LaunchAfterInstall=Launch DeepSeek Harness-rs
chinesesimp.DesktopShortcut=创建桌面快捷方式
chinesesimp.AdditionalTasks=附加任务：
chinesesimp.LauncherName=DeepSeek Harness-rs 启动器
chinesesimp.LaunchAfterInstall=启动 DeepSeek Harness-rs
[Tasks]
Name: "desktopicon"; Description: "{cm:DesktopShortcut}"; GroupDescription: "{cm:AdditionalTasks}"; Flags: unchecked
[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\dsh-launcher.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\dsh-launcher.exe"; WorkingDir: "{app}"; Tasks: desktopicon
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
[Run]
Filename: "{app}\dsh-launcher.exe"; Description: "{cm:LaunchAfterInstall}"; Flags: postinstall nowait skipifsilent
