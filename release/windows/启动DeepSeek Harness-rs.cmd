@echo off
setlocal EnableExtensions
set "PWSH=C:\Program Files\PowerShell\7\pwsh.exe"
set "MANAGER=%~dp0DshServiceManager.ps1"
if exist "%PWSH%" goto launch
cls
echo ============================================================
echo   DeepSeek Harness-rs 需要 PowerShell 7
echo ============================================================
echo.
echo 未找到：%PWSH%
echo 请在管理员 PowerShell 中执行：
echo winget install --id Microsoft.PowerShell --source winget
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
