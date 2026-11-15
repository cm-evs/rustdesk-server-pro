@echo off
setlocal
rem ============================================================
rem  把 RustDesk 中继代理注册为 Windows 服务（开机自启）
rem  服务是 relay-agent.exe：它负责监督 hbbr、心跳与收配置。
rem  注意：先注册节点，再启动服务（未注册时服务会直接退出）。
rem ============================================================
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
if not exist "%~dp0hbbr.exe" (
  echo 找不到 hbbr.exe，请把本脚本放在中继包目录里运行。
  pause
  exit /b 1
)
"%~dp0relay-agent.exe" install
if %errorlevel% neq 0 (
  echo 注册失败，请看上面的错误信息。
  pause
  exit /b 1
)
rem 服务自身异常退出时，由系统自动重启它（hbbr 由代理守护）
sc.exe failure RustDeskRelayAgent reset= 86400 actions= restart/60000/restart/60000/restart/60000 >nul
echo.
echo 服务已注册（尚未启动）。
if not exist "%~dp0agent.json" (
  echo 下一步：先注册节点，再启动服务：
  echo   relay-agent.exe register --link ^<控制台设备页生成的注册链接^>
  echo   relay-agent.exe start
) else (
  echo 节点已注册，正在启动服务……
  "%~dp0relay-agent.exe" start
)
echo 日常启停用 service.bat，或在 Windows 的「服务」里找 RustDesk Relay Agent。
echo 注意：被管中继上只由代理启动 hbbr，不要再手动运行 hbbr.exe，两边会抢占端口。
pause
endlocal
