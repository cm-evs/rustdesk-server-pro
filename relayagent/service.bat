@echo off
setlocal
rem ============================================================
rem  RustDesk 中继代理管理：启动 / 停止 / 重启 / 状态
rem  已注册 Windows 服务时管服务；没注册时按进程管
rem  （启动走 relay-agent run，停止直接结束 relay-agent/hbbr 进程）。
rem  双击运行显示菜单；也可带参数：service.bat start|stop|restart|status
rem ============================================================
cd /d "%~dp0"
if /i "%~2"=="menu" set "PAUSEME=1"
set "ACTION=%~1"
if not "%ACTION%"=="" goto dispatch
:menu
echo.
echo ===== RustDesk 中继代理管理 =====
echo   1. 启动
echo   2. 停止
echo   3. 重启
echo   4. 查看状态
echo   5. 退出
set "SEL="
set /p "SEL=请选择 [1-5]，然后按回车："
set "SEL=%SEL: =%"
if "%SEL%"=="1" set "ACTION=start"
if "%SEL%"=="2" set "ACTION=stop"
if "%SEL%"=="3" set "ACTION=restart"
if "%SEL%"=="4" set "ACTION=status"
if "%SEL%"=="5" exit /b 0
if "%ACTION%"=="" (
  echo 输入无效，请输入 1 到 5。
  goto menu
)
:dispatch
echo 操作：%ACTION%
if /i "%ACTION%"=="status" goto do_status
rem 启动/停止/重启需要管理员权限（服务方式必须，进程方式也一并提权）
net session >nul 2>&1
if %errorlevel% neq 0 (
  echo 需要管理员权限，正在申请提权……
  powershell -NoProfile -ExecutionPolicy Bypass -Command "Start-Process -FilePath '%~f0' -ArgumentList '%ACTION%','menu' -Verb RunAs"
  exit /b
)
sc.exe query RustDeskRelayAgent >nul 2>&1
if %errorlevel% equ 0 goto svc_mode
goto proc_mode
:svc_mode
if /i "%ACTION%"=="start" "%~dp0relay-agent.exe" start
if /i "%ACTION%"=="stop" "%~dp0relay-agent.exe" stop
if /i "%ACTION%"=="restart" "%~dp0relay-agent.exe" restart
goto done
:proc_mode
echo 服务未安装，按进程方式管理（注册服务请运行 install-service.bat）。
if /i "%ACTION%"=="start" goto proc_start
if /i "%ACTION%"=="stop" goto proc_stop
if /i "%ACTION%"=="restart" goto proc_restart
goto done
:proc_start
tasklist /FI "IMAGENAME eq relay-agent.exe" 2>nul | find /I "relay-agent.exe" >nul
if %errorlevel% equ 0 (
  echo relay-agent 已经在运行，不重复启动。
  goto done
)
start "RustDesk Relay Agent" /D "%~dp0" "%~dp0relay-agent.exe" run
echo 已打开代理窗口（relay-agent run 方式启动；未注册时会直接退出，请先 register）。
goto done
:proc_stop
taskkill /IM relay-agent.exe /F >nul 2>&1
taskkill /IM hbbr.exe /F >nul 2>&1
echo 已停止 relay-agent / hbbr 进程（如果它们在运行的话）。
goto done
:proc_restart
taskkill /IM relay-agent.exe /F >nul 2>&1
taskkill /IM hbbr.exe /F >nul 2>&1
timeout /T 2 /NOBREAK >nul
start "RustDesk Relay Agent" /D "%~dp0" "%~dp0relay-agent.exe" run
echo 已按进程方式重启。
goto done
:do_status
sc.exe query RustDeskRelayAgent >nul 2>&1
if %errorlevel% equ 0 (
  "%~dp0relay-agent.exe" status
) else (
  echo Windows 服务：未安装（可用 install-service.bat 注册）
)
echo --- 进程 ---
tasklist /FI "IMAGENAME eq relay-agent.exe"
tasklist /FI "IMAGENAME eq hbbr.exe"
echo --- 注册信息 ---
"%~dp0relay-agent.exe" info
goto done
:done
if "%PAUSEME%"=="1" pause
if "%~1"=="" pause
endlocal
