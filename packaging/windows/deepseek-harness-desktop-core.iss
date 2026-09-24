#ifndef SourceDir
#error SourceDir must point to the verified desktop and core payload
#endif
#ifndef OutputDir
#error OutputDir must point to the installer output directory
#endif
#ifndef PackageRevision
#define PackageRevision "r13"
#endif

#define MyAppName "DeepSeek Harness Desktop (Preview)"
#define MyAppVersion "0.1.3-alpha.34 Flutter " + PackageRevision
#define MyAppId "{{37BA446F-D181-493B-9703-23978B3C194A}"

#if !FileExists(SourceDir + "\dsh_desktop.exe")
#error The Flutter desktop executable is missing
#endif
#if !FileExists(SourceDir + "\data\app.so")
#error The Flutter application bundle is missing
#endif
#if !FileExists(SourceDir + "\flutter_windows.dll")
#error The Flutter runtime is missing
#endif
#if !FileExists(SourceDir + "\host\deepseek-harness-rs.exe")
#error The Rust core executable is missing
#endif
#if !FileExists(SourceDir + "\host\PACKAGE.json")
#error The Rust core package inventory is missing
#endif
#if !FileExists(SourceDir + "\host\web\dist\index.html")
#error The bundled core web resources are missing
#endif
#if !FileExists(SourceDir + "\host\runtime\node\node.exe")
#error The bundled Node runtime is missing
#endif
#if !FileExists(SourceDir + "\host\runtime\native-install-upgrade.cjs")
#error The native sandbox upgrade helper is missing
#endif

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher=DeepSeek Harness-rs
DefaultDirName={localappdata}\Programs\DeepSeek Harness Desktop
UsePreviousAppDir=yes
DefaultGroupName={#MyAppName}
OutputDir={#OutputDir}
OutputBaseFilename=deepseek-harness-desktop-core-20260923-{#PackageRevision}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=lowest
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
SetupIconFile=deepseek-black.ico
UninstallDisplayIcon={app}\dsh_desktop.exe
ShowLanguageDialog=auto
LanguageDetectionMethod=uilanguage
CloseApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "chinesesimp"; MessagesFile: "ChineseSimplified.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:DesktopShortcut}"; GroupDescription: "{cm:AdditionalTasks}"; Flags: unchecked

[CustomMessages]
english.DesktopShortcut=Create a desktop shortcut
english.AdditionalTasks=Additional tasks:
english.LaunchAfterInstall=Launch DeepSeek Harness Desktop
english.NativeUpgradeFailed=Native sandbox upgrade verification failed. Check the installation log.
chinesesimp.DesktopShortcut=创建桌面快捷方式
chinesesimp.AdditionalTasks=附加任务：
chinesesimp.LaunchAfterInstall=启动 DeepSeek Harness Desktop
chinesesimp.NativeUpgradeFailed=原生沙箱升级校验失败，请查看安装日志。

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\dsh_desktop.exe"; WorkingDir: "{app}"; IconFilename: "{app}\dsh_desktop.exe"; IconIndex: 0
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\dsh_desktop.exe"; WorkingDir: "{app}"; IconFilename: "{app}\dsh_desktop.exe"; IconIndex: 0; Tasks: desktopicon

[Run]
Filename: "{app}\dsh_desktop.exe"; Description: "{cm:LaunchAfterInstall}"; Flags: postinstall nowait skipifsilent

[Code]
procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep = ssPostInstall then
  begin
    ResultCode := -1;
    if not Exec(ExpandConstant('{app}\host\runtime\node\node.exe'),
      '"' + ExpandConstant('{app}\host\runtime\native-install-upgrade.cjs') + '"',
      ExpandConstant('{app}\host'), SW_HIDE, ewWaitUntilTerminated, ResultCode) then
      RaiseException(CustomMessage('NativeUpgradeFailed'));
    if ResultCode <> 0 then
      RaiseException(CustomMessage('NativeUpgradeFailed'));
  end;
end;
