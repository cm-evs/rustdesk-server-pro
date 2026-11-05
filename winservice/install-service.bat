@echo off
setlocal
rem ============================================================
rem  把 RustDesk 服务端注册为 Windows 服务（开机自启）
rem  服务宿主是 rdsvc.exe：hbbs/hbbr 本身不懂服务协议，
rem  由它负责拉起、守护和停止两个进程。
rem ============================================================
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
"%~dp0rdsvc.exe" install
if %errorlevel% neq 0 (
  echo 注册失败，请看上面的错误信息。
  pause
  exit /b 1
)
rem 服务宿主自身异常退出时，由系统自动重启它（子进程由宿主守护）
sc.exe failure RustDeskPro reset= 86400 actions= restart/60000/restart/60000/restart/60000 >nul
echo 正在启动服务……
"%~dp0rdsvc.exe" start
echo.
echo 完成。服务端已作为 Windows 服务运行并开机自启。
echo 日常启停用 service.bat，或在 Windows 的「服务」里找 RustDesk Server Pro。
echo 注意：改用服务后不要再双击 start.bat，两边会抢占端口。
pause
endlocal
