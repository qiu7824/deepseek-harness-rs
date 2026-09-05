#ifndef MyAppVersion
#define MyAppVersion "0.1.3-alpha.5"
#endif
#ifndef SourceDir
#define SourceDir "dist\deepseek-harness-rs-v0.1.3-alpha.5-windows-x86_64-core"
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
#if !FileExists(SourceDir + "\dsh-launcher.exe")
#error The installer payload is missing dsh-launcher.exe
#endif
#if !FileExists(SourceDir + "\deepseek-harness-rs.exe")
#error The installer payload is missing deepseek-harness-rs.exe
#endif
#if !FileExists(SourceDir + "\PACKAGE.json")
#error The installer payload is missing PACKAGE.json
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
DefaultDirName=D:\Program Files (x86)\DeepSeek Harness-rs\{#Variant}
UsePreviousAppDir=yes
DisableDirPage=no
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
english.DirectoryUnavailable=The selected drive or folder is unavailable. Please choose another installation folder.
english.DirectoryNotWritable=The selected folder is not writable. Choose a folder you can write to, or restart Setup with appropriate permissions.
english.IncompleteInstallation=The launcher or core program is missing from the installation folder. The application cannot start. Reinstall using a complete installation package.
chinesesimp.DesktopShortcut=创建桌面快捷方式
chinesesimp.AdditionalTasks=附加任务：
chinesesimp.LauncherName=DeepSeek Harness-rs 启动器
chinesesimp.LaunchAfterInstall=启动 DeepSeek Harness-rs
chinesesimp.DirectoryUnavailable=所选磁盘或文件夹不可用，请选择其他安装目录。
chinesesimp.DirectoryNotWritable=无法写入所选目录，请选择有写入权限的目录，或使用适当权限重新运行安装程序。
chinesesimp.IncompleteInstallation=安装目录中缺少启动器或核心程序，无法启动。请使用完整安装包重新安装。
[Tasks]
Name: "desktopicon"; Description: "{cm:DesktopShortcut}"; GroupDescription: "{cm:AdditionalTasks}"; Flags: unchecked
[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\dsh-launcher.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\dsh-launcher.exe"; WorkingDir: "{app}"; Tasks: desktopicon
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
[Run]
Filename: "{app}\dsh-launcher.exe"; Description: "{cm:LaunchAfterInstall}"; Flags: postinstall nowait skipifsilent; Check: InstalledRuntimeReady

[Code]
function InstalledRuntimeReady: Boolean;
begin
  Result := FileExists(ExpandConstant('{app}\dsh-launcher.exe')) and
    FileExists(ExpandConstant('{app}\deepseek-harness-rs.exe')) and
    FileExists(ExpandConstant('{app}\PACKAGE.json'));
  if not Result then
    MsgBox(CustomMessage('IncompleteInstallation'), mbError, MB_OK);
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
  { Test only the nearest existing parent; never create the selected app tree
    before the user starts installation. Remove only our fresh empty probe. }
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
  if (CurPageID = wpSelectDir) and not WizardSilent then
  begin
    Failure := CheckInstallDirectory;
    if Failure <> '' then
    begin
      MsgBox(Failure, mbError, MB_OK);
      Result := False;
    end;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  { Silent installs also fail explicitly; no hidden fallback to another drive. }
  Result := CheckInstallDirectory;
end;
