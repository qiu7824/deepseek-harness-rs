@echo off
setlocal EnableExtensions
set "PWSH=C:\Program Files\PowerShell\7\pwsh.exe"
set "MANAGER=%~dp0DshServiceManager.ps1"

if exist "%PWSH%" goto launch

cls
echo ============================================================
echo   DeepSeek Harness 运行管理器需要 PowerShell 7
echo ============================================================
echo.
echo 未找到：%PWSH%
echo.
echo 请在管理员 CMD 或 Windows PowerShell 中执行以下一键安装命令：
echo.
set "INSTALL=powershell -NoProfile -ExecutionPolicy Bypass -Command "$u='https://github.com/PowerShell/PowerShell/releases/download/v7.6.5/PowerShell-7.6.5-win-x64.msi'; $p='$env:TEMP\PowerShell-7.6.5-win-x64.msi'; Invoke-WebRequest $u -OutFile $p; Start-Process msiexec.exe -Verb RunAs -Wait -ArgumentList '/i',$p,'/qn','ADD_EXPLORER_CONTEXT_MENU_OPENPOWERSHELL=1','ENABLE_PSREMOTING=1','REGISTER_MANIFEST=1'; Remove-Item $p -Force""
echo %INSTALL%
echo.
<nul set /p="%INSTALL%" | clip.exe
echo 安装命令已复制到剪贴板。
echo 安装完成后，请重新双击本文件。
echo.
pause
exit /b 1

:launch
if not exist "%MANAGER%" (
  echo 未找到运行管理器：%MANAGER%
  pause
  exit /b 1
)
powershell.exe -NoProfile -WindowStyle Hidden -Command "Start-Process -FilePath '%PWSH%' -WindowStyle Hidden -ArgumentList '-NoProfile','-STA','-File','\"%MANAGER%\"'"
exit /b 0
