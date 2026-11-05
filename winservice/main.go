// rdsvc — Windows service host for rustdesk-server-pro.
//
// hbbs.exe / hbbr.exe are plain console programs and do not speak the
// Windows Service Control Manager (SCM) protocol, so they cannot be
// registered with sc.exe directly (start would fail with error 1053).
// rdsvc.exe is the thin service that SCM talks to: it reads the same
// server.env as start.ps1, launches hbbr + hbbs with the same command
// lines start.ps1 builds, restarts a child that dies unexpectedly, and
// stops both children when the service is stopped.
//
// Usage:
//
//	rdsvc.exe            run as the service (only valid under SCM)
//	rdsvc.exe install    register the service (needs admin)
//	rdsvc.exe uninstall  stop + remove the service (needs admin)
//	rdsvc.exe start      start the service
//	rdsvc.exe stop       stop the service
//	rdsvc.exe restart    stop (if running) + start
//	rdsvc.exe status     print the service state
package main

import (
	"bufio"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/svc"
	"golang.org/x/sys/windows/svc/mgr"
)

const svcName = "RustDeskPro"
const svcDisplay = "RustDesk Server Pro"
const svcDesc = "RustDesk 服务端（hbbs + hbbr），由 rdsvc.exe 宿主。账号控制台/API 与中继服务。"

// ---------------------------------------------------------------------------
// paths + logging

func exeDir() string {
	exe, err := os.Executable()
	if err != nil {
		return "."
	}
	return filepath.Dir(exe)
}

var logMu sync.Mutex

func svcLog(format string, a ...interface{}) {
	logMu.Lock()
	defer logMu.Unlock()
	dir := filepath.Join(exeDir(), "logs")
	_ = os.MkdirAll(dir, 0o755)
	f, err := os.OpenFile(filepath.Join(dir, "rdsvc.log"),
		os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return
	}
	defer f.Close()
	line := time.Now().Format("2006-01-02 15:04:05") + " " +
		fmt.Sprintf(format, a...) + "\r\n"
	_, _ = f.WriteString(line)
}

// ---------------------------------------------------------------------------
// server.env — same parsing rules as start.ps1: KEY=value lines, '#'
// comments and blank lines skipped, empty values never override a
// default, one pair of surrounding quotes stripped.

type serverConfig struct {
	hbbsPort      string
	hbbrPort      string
	apiPort       string
	relayServers  string
	consoleListen string
	tlsCert       string
	tlsKey        string
}

func loadConfig(dir string) serverConfig {
	cfg := serverConfig{hbbsPort: "21116", hbbrPort: "21117", apiPort: "21114"}
	f, err := os.Open(filepath.Join(dir, "server.env"))
	if err != nil {
		return cfg
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 64*1024), 64*1024)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		idx := strings.Index(line, "=")
		if idx <= 0 {
			continue
		}
		key := strings.TrimSpace(line[:idx])
		val := strings.TrimSpace(line[idx+1:])
		if val == "" {
			continue
		}
		if len(val) >= 2 {
			first, last := val[0], val[len(val)-1]
			if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
				val = val[1 : len(val)-1]
			}
		}
		switch key {
		case "HBBS_PORT":
			cfg.hbbsPort = val
		case "HBBR_PORT":
			cfg.hbbrPort = val
		case "API_PORT":
			cfg.apiPort = val
		case "RELAY_SERVERS":
			cfg.relayServers = val
		case "CONSOLE_LISTEN":
			cfg.consoleListen = val
		case "TLS_CERT":
			cfg.tlsCert = val
		case "TLS_KEY":
			cfg.tlsKey = val
		}
	}
	return cfg
}

func (c serverConfig) hbbrArgs() []string {
	return []string{"-p", c.hbbrPort}
}

func (c serverConfig) hbbsArgs() []string {
	args := []string{"-p", c.hbbsPort, "--api-listen", "0.0.0.0:" + c.apiPort}
	if c.relayServers != "" {
		args = append(args, "--relay-servers", c.relayServers)
	}
	if c.consoleListen != "" {
		args = append(args, "--console-listen", c.consoleListen)
	}
	if c.tlsCert != "" {
		args = append(args, "--tls-cert", c.tlsCert, "--tls-key", c.tlsKey)
	}
	return args
}

// ---------------------------------------------------------------------------
// child supervision

type supervisor struct {
	dir  string
	cfg  serverConfig
	stop chan struct{}
	mu   sync.Mutex
	cmds map[string]*exec.Cmd
}

func newSupervisor(dir string) *supervisor {
	return &supervisor{
		dir:  dir,
		cfg:  loadConfig(dir),
		stop: make(chan struct{}),
		cmds: make(map[string]*exec.Cmd),
	}
}

func (s *supervisor) stopped() bool {
	select {
	case <-s.stop:
		return true
	default:
		return false
	}
}

// runChild starts one child and keeps it alive: an unexpected exit is
// logged and the child is restarted after a short delay, until the
// service itself is stopped.
func (s *supervisor) runChild(name string, args []string) {
	exe := filepath.Join(s.dir, name)
	for {
		if s.stopped() {
			return
		}
		cmd := exec.Command(exe, args...)
		cmd.Dir = s.dir
		cmd.Env = append(os.Environ(), "TEST_HBBS=no")
		out, err := os.OpenFile(
			filepath.Join(s.dir, "logs", "rdsvc-"+name+".log"),
			os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
		if err == nil {
			cmd.Stdout = out
			cmd.Stderr = out
		}
		if err := cmd.Start(); err != nil {
			svcLog("%s 启动失败: %v（10 秒后重试）", name, err)
			if out != nil {
				out.Close()
			}
			select {
			case <-s.stop:
				return
			case <-time.After(10 * time.Second):
			}
			continue
		}
		s.mu.Lock()
		s.cmds[name] = cmd
		s.mu.Unlock()
		svcLog("%s 已启动 pid=%d args=%s", name, cmd.Process.Pid,
			strings.Join(args, " "))
		waitErr := cmd.Wait()
		if out != nil {
			out.Close()
		}
		s.mu.Lock()
		delete(s.cmds, name)
		s.mu.Unlock()
		if s.stopped() {
			return
		}
		svcLog("%s 意外退出: %v（3 秒后重启）", name, waitErr)
		select {
		case <-s.stop:
			return
		case <-time.After(3 * time.Second):
		}
	}
}

func (s *supervisor) shutdown() {
	close(s.stop)
	s.mu.Lock()
	defer s.mu.Unlock()
	for name, cmd := range s.cmds {
		if cmd.Process != nil {
			svcLog("停止 %s pid=%d", name, cmd.Process.Pid)
			_ = cmd.Process.Kill()
		}
	}
}

// ---------------------------------------------------------------------------
// SCM handler

type rdService struct{}

func (rdService) Execute(_ []string, r <-chan svc.ChangeRequest,
	changes chan<- svc.Status) (bool, uint32) {
	changes <- svc.Status{State: svc.StartPending}
	sup := newSupervisor(exeDir())
	go sup.runChild("hbbr.exe", sup.cfg.hbbrArgs())
	go sup.runChild("hbbs.exe", sup.cfg.hbbsArgs())
	changes <- svc.Status{
		State:   svc.Running,
		Accepts: svc.AcceptStop | svc.AcceptShutdown,
	}
	svcLog("服务已启动（hbbr 端口 %s，hbbs 端口 %s，API 端口 %s）",
		sup.cfg.hbbrPort, sup.cfg.hbbsPort, sup.cfg.apiPort)
	for req := range r {
		switch req.Cmd {
		case svc.Interrogate:
			changes <- req.CurrentStatus
		case svc.Stop, svc.Shutdown:
			svcLog("收到停止请求，正在停止子进程")
			changes <- svc.Status{State: svc.StopPending}
			sup.shutdown()
			// Give the Wait() calls a moment to reap the children.
			time.Sleep(500 * time.Millisecond)
			changes <- svc.Status{State: svc.Stopped}
			svcLog("服务已停止")
			return false, 0
		}
	}
	return false, 0
}

// ---------------------------------------------------------------------------
// install / uninstall / control — thin wrappers over svc/mgr

func connectMgr() (*mgr.Mgr, error) {
	m, err := mgr.Connect()
	if err != nil {
		return nil, fmt.Errorf("连接服务管理器失败（请以管理员身份运行）: %w", err)
	}
	return m, nil
}

func doInstall() error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	m, err := connectMgr()
	if err != nil {
		return err
	}
	defer m.Disconnect()
	cfg := mgr.Config{
		ServiceType: 0x10, // SERVICE_WIN32_OWN_PROCESS
		StartType:   mgr.StartAutomatic,
		DisplayName: svcDisplay,
		Description: svcDesc,
	}
	if s, err := m.OpenService(svcName); err == nil {
		// Already registered: just refresh the binary path + config,
		// so moving the server directory does not break the service.
		defer s.Close()
		if err := s.UpdateConfig(cfg); err != nil {
			return fmt.Errorf("更新已有服务配置失败: %w", err)
		}
		fmt.Printf("服务 %s 已存在，已更新程序路径为 %s\n", svcName, exe)
		return nil
	}
	s, err := m.CreateService(svcName, exe, cfg)
	if err != nil {
		return fmt.Errorf("创建服务失败: %w", err)
	}
	defer s.Close()
	fmt.Printf("服务 %s 已注册（开机自启），程序路径 %s\n", svcName, exe)
	return nil
}

func doUninstall() error {
	m, err := connectMgr()
	if err != nil {
		return err
	}
	defer m.Disconnect()
	s, err := m.OpenService(svcName)
	if err != nil {
		fmt.Printf("服务 %s 未安装，无需卸载。\n", svcName)
		return nil
	}
	defer s.Close()
	_, _ = s.Control(svc.Stop)
	deadline := time.Now().Add(15 * time.Second)
	for time.Now().Before(deadline) {
		st, err := s.Query()
		if err != nil || st.State == svc.Stopped {
			break
		}
		time.Sleep(300 * time.Millisecond)
	}
	if err := s.Delete(); err != nil {
		return fmt.Errorf("删除服务失败: %w", err)
	}
	fmt.Printf("服务 %s 已卸载。\n", svcName)
	return nil
}

func openSvc() (*mgr.Service, error) {
	m, err := connectMgr()
	if err != nil {
		return nil, err
	}
	s, err := m.OpenService(svcName)
	if err != nil {
		m.Disconnect()
		return nil, fmt.Errorf("服务 %s 未安装（先运行 install-service.bat）", svcName)
	}
	// Note: the mgr handle is intentionally left connected; closing
	// the service handle is enough for this short-lived CLI process.
	return s, nil
}

func stateName(st svc.State) string {
	switch st {
	case svc.Stopped:
		return "已停止"
	case svc.StartPending:
		return "正在启动"
	case svc.StopPending:
		return "正在停止"
	case svc.Running:
		return "运行中"
	case svc.ContinuePending:
		return "正在继续"
	case svc.PausePending:
		return "正在暂停"
	case svc.Paused:
		return "已暂停"
	}
	return fmt.Sprintf("未知(%d)", int(st))
}

func waitState(s *mgr.Service, want svc.State, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		st, err := s.Query()
		if err != nil {
			return err
		}
		if st.State == want {
			return nil
		}
		time.Sleep(300 * time.Millisecond)
	}
	st, _ := s.Query()
	return fmt.Errorf("等待超时，当前状态：%s", stateName(st.State))
}

func doStart() error {
	s, err := openSvc()
	if err != nil {
		return err
	}
	defer s.Close()
	st, err := s.Query()
	if err == nil && st.State == svc.Running {
		fmt.Println("服务已经在运行中。")
		return nil
	}
	if err := s.Start(); err != nil {
		return fmt.Errorf("启动失败: %w", err)
	}
	if err := waitState(s, svc.Running, 30*time.Second); err != nil {
		return err
	}
	fmt.Println("服务已启动。")
	return nil
}

func doStop() error {
	s, err := openSvc()
	if err != nil {
		return err
	}
	defer s.Close()
	st, err := s.Query()
	if err == nil && st.State == svc.Stopped {
		fmt.Println("服务已经是停止状态。")
		return nil
	}
	if _, err := s.Control(svc.Stop); err != nil {
		return fmt.Errorf("停止失败: %w", err)
	}
	if err := waitState(s, svc.Stopped, 30*time.Second); err != nil {
		return err
	}
	fmt.Println("服务已停止。")
	return nil
}

func doStatus() error {
	s, err := openSvc()
	if err != nil {
		return err
	}
	defer s.Close()
	st, err := s.Query()
	if err != nil {
		return err
	}
	fmt.Printf("服务 %s 状态：%s\n", svcName, stateName(st.State))
	return nil
}

const usage = `rdsvc — RustDesk Server Pro 的 Windows 服务宿主

用法:
  rdsvc.exe install     注册为 Windows 服务（开机自启，需管理员）
  rdsvc.exe uninstall   停止并卸载服务（需管理员）
  rdsvc.exe start       启动服务
  rdsvc.exe stop        停止服务
  rdsvc.exe restart     重启服务
  rdsvc.exe status      查看服务状态
  （无参数是给 Windows 服务管理器用的，不要直接双击运行）
`

func main() {
	if len(os.Args) < 2 {
		// Launched by SCM — or by a curious double-click, in which
		// case svc.Run errors out and we explain instead of vanishing.
		if err := svc.Run(svcName, rdService{}); err != nil {
			fmt.Println("无法以服务方式运行（本程序应由 Windows 服务管理器启动）。")
			fmt.Println()
			fmt.Print(usage)
			time.Sleep(3 * time.Second)
			os.Exit(1)
		}
		return
	}
	// CLI mode: the messages below are UTF-8 Chinese; switch the
	// console to UTF-8 so they do not turn into mojibake on a GBK
	// (cp936) console. Harmless when output is redirected.
	_ = windows.SetConsoleCP(65001)
	_ = windows.SetConsoleOutputCP(65001)
	var err error
	switch strings.ToLower(os.Args[1]) {
	case "install":
		err = doInstall()
	case "uninstall":
		err = doUninstall()
	case "start":
		err = doStart()
	case "stop":
		err = doStop()
	case "restart":
		if e := doStop(); e != nil {
			err = e
		} else {
			err = doStart()
		}
	case "status":
		err = doStatus()
	default:
		fmt.Print(usage)
		return
	}
	if err != nil {
		fmt.Println("错误：", err)
		os.Exit(1)
	}
}
