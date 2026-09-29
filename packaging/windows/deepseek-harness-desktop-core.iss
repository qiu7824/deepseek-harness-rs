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
#ifndef MyAppVersion
#define MyAppVersion "0.1.3-alpha.38"
#endif
#define MyAppId "{{37BA446F-D181-493B-9703-23978B3C194A}"
#define DshTitle "DeepSeek Harness Desktop"
#define DshAppSubdir "DeepSeek Harness-rs\desktop"
#ifndef ArtDir
#define ArtDir SourcePath + "installer"
#endif

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
DefaultDirName=D:\Program Files (x86)\DeepSeek Harness-rs\desktop
UsePreviousAppDir=yes
DisableDirPage=no
DisableWelcomePage=yes
DisableReadyPage=yes
DisableProgramGroupPage=yes
WizardResizable=no
WizardSizePercent=100
DefaultGroupName={#MyAppName}
OutputDir={#OutputDir}
OutputBaseFilename=deepseek-harness-rs-v{#MyAppVersion}-windows-x86_64-flutter-setup
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
english.DirectoryUnavailable=The selected drive or folder is unavailable. Please choose another installation folder.
english.DirectoryNotWritable=The selected folder is not writable. Choose a folder you can write to, or restart Setup with appropriate permissions.
chinesesimp.DesktopShortcut=创建桌面快捷方式
chinesesimp.AdditionalTasks=附加任务：
chinesesimp.LaunchAfterInstall=启动 DeepSeek Harness Desktop
chinesesimp.NativeUpgradeFailed=原生沙箱升级校验失败，请查看安装日志。
chinesesimp.DirectoryUnavailable=所选磁盘或文件夹不可用，请选择其他安装目录。
chinesesimp.DirectoryNotWritable=无法写入所选目录，请选择有写入权限的目录，或使用适当权限重新运行安装程序。

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
#include ArtDir + "\installer-ui.iss"

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

function CheckInstallDirectory: String;
var
  Directory, ExistingParent, Probe: String;
  Attempt: Integer;
begin
  Result := '';
  Directory := ExpandFileName(WizardDirValue);
  ExistingParent := Directory;
  while not DirExists(ExistingParent) do
  begin
    if FileExists(ExistingParent) or (ExistingParent = '') or
       (ExtractFileDir(ExistingParent) = ExistingParent) then
    begin
      Result := CustomMessage('DirectoryUnavailable');
      Exit;
    end;
    ExistingParent := ExtractFileDir(ExistingParent);
  end;
  { Probe the nearest existing parent without creating the app tree. }
  for Attempt := 1 to 100 do
  begin
    Probe := AddBackslash(ExistingParent) + '.dsh-install-check-' +
      IntToStr(Random(2147483647));
    if not DirExists(Probe) and not FileExists(Probe) then
    begin
      if not CreateDir(Probe) then
        Result := CustomMessage('DirectoryNotWritable')
      else if not RemoveDir(Probe) then
        Result := CustomMessage('DirectoryNotWritable');
      Exit;
    end;
  end;
  Result := CustomMessage('DirectoryNotWritable');
end;

function NextButtonClick(CurPageID: Integer): Boolean;
var
  Failure: String;
begin
  Result := True;
  if DshIsLanding(CurPageID) and not WizardSilent then
  begin
    DshApplyChoices;
    Failure := CheckInstallDirectory;
    if Failure <> '' then
    begin
      DshShowError(Failure);
      Result := False;
    end;
  end;
end;

{ The desktop client starts its bundled Host detached, so it outlives the
  window. Stop only the Host that runs from this installation before the
  upgrade replaces it; a new client would otherwise attach to the previous
  version. A separately installed Web core runs from another path. }
procedure StopBundledHost;
var
  Host, Script: String;
  ResultCode: Integer;
begin
  Host := ExpandConstant('{app}\host\deepseek-harness-rs.exe');
  if not FileExists(Host) then Exit;
  StringChangeEx(Host, '''', '''''', True);
  Script := '$p = ''' + Host + '''; Get-Process -Name deepseek-harness-rs -ErrorAction SilentlyContinue | ' +
    'Where-Object { $_.Path -ieq $p } | Stop-Process -Force -ErrorAction SilentlyContinue';
  Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "' + Script + '"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Result := CheckInstallDirectory;
  if Result = '' then StopBundledHost;
end;
