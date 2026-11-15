@echo off
setlocal
rem 停止并卸载 RustDeskRelayAgent Windows 服务（数据文件不受影响）
net session >nul 2>&1
if %errorlevel% neq 0 (
  echo 需要管理员权限，正在申请提权……
  powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process -FilePath '%~f0' -Verb RunAs"
  exit /b
)
cd /d "%~dp0"
if not exist "%~dp0relay-agent.exe" (
  echo 找不到 relay-agent.exe，请把本脚本放在中继包目录里运行。
  pause
  exit /b 1
)
"%~dp0relay-agent.exe" uninstall
echo.
echo 卸载完成。中继包目录与数据文件都还在，之后仍可手动运行 relay-agent.exe run。
pause
endlocal
