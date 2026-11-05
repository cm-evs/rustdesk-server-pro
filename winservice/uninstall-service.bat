@echo off
setlocal
rem 停止并卸载 RustDeskPro Windows 服务（数据文件不受影响）
net session >nul 2>&1
if %errorlevel% neq 0 (
  echo 需要管理员权限，正在申请提权……
  powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
  exit /b
)
cd /d "%~dp0"
if not exist "%~dp0rdsvc.exe" (
  echo 找不到 rdsvc.exe，请把本脚本放在服务端目录里运行。
  pause
  exit /b 1
)
"%~dp0rdsvc.exe" uninstall
echo.
echo 卸载完成。服务端目录与数据文件都还在，之后仍可双击 start.bat 方式运行。
pause
endlocal
