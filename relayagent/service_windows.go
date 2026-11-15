//go:build windows

package main

import (
	"fmt"
	"os"
	"time"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/svc"
	"golang.org/x/sys/windows/svc/mgr"
)

const svcName = "RustDeskRelayAgent"
const svcDisplay = "RustDesk Relay Agent"
const svcDesc = "RustDesk 中继节点代理：向服务端注册并心跳，管理本地 hbbr 中继进程。"

// runDefault is the no-argument behaviour on Windows: run as a
// service under the Service Control Manager.
func runDefault() int {
	if err := svc.Run(svcName, relayService{}); err != nil {
		fmt.Println("无法以服务方式运行（本程序应由 Windows 服务管理器启动，或用 run 命令前台运行）。")
		fmt.Println()
		fmt.Print(usage)
		time.Sleep(3 * time.Second)
		return 1
	}
	return 0
}

type relayService struct{}

func (relayService) Execute(_ []string, r <-chan svc.ChangeRequest, changes chan<- svc.Status) (bool, uint32) {
	changes <- svc.Status{State: svc.StartPending}
	agent, err := buildAgent(exeDir(), false)
	if err != nil {
		// Log through a throwaway logger: the agent failed before
		// its own logger existed.
		newLogger(exeDir(), false).Logf("服务启动失败：%v", err)
		changes <- svc.Status{State: svc.Stopped}
		return false, 1
	}
	stopCh := make(chan struct{})
	done := make(chan struct{})
	go func() {
		agent.Run(stopCh)
		close(done)
	}()
	changes <- svc.Status{State: svc.Running, Accepts: svc.AcceptStop | svc.AcceptShutdown}
	for req := range r {
		switch req.Cmd {
		case svc.Interrogate:
			changes <- req.CurrentStatus
		case svc.Stop, svc.Shutdown:
			close(stopCh)
			changes <- svc.Status{State: svc.StopPending}
			<-done
			changes <- svc.Status{State: svc.Stopped}
			return false, 0
		}
	}
	return false, 0
}

func doServiceCommand(cmd string) int {
	// CLI messages are UTF-8 Chinese; switch the console to UTF-8 so
	// they do not turn into mojibake on a GBK (cp936) console.
	_ = windows.SetConsoleCP(65001)
	_ = windows.SetConsoleOutputCP(65001)
	var err error
	switch cmd {
	case "install":
		err = svcInstall()
	case "uninstall":
		err = svcUninstall()
	case "start":
		err = svcStart()
	case "stop":
		err = svcStop()
	case "restart":
		if e := svcStop(); e != nil {
			err = e
		} else {
			err = svcStart()
		}
	case "status":
		err = svcStatus()
	}
	if err != nil {
		fmt.Println("错误：", err)
		return 1
	}
	return 0
}

func connectMgr() (*mgr.Mgr, error) {
	m, err := mgr.Connect()
	if err != nil {
		return nil, fmt.Errorf("连接服务管理器失败（请以管理员身份运行）: %w", err)
	}
	return m, nil
}

func svcInstall() error {
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
		// so moving the directory does not break the service.
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

func svcUninstall() error {
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
		return nil, fmt.Errorf("服务 %s 未安装（先运行 relay-agent install）", svcName)
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

func svcStart() error {
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

func svcStop() error {
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

func svcStatus() error {
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
