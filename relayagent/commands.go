package main

import (
	"fmt"
	"os"
	"os/signal"
)

// runForeground runs the agent in the foreground until Ctrl-C/SIGTERM.
func runForeground() int {
	agent, err := buildAgent(exeDir(), true)
	if err != nil {
		fmt.Println("错误：", err)
		return 1
	}
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, shutdownSignals()...)
	stopCh := make(chan struct{})
	done := make(chan struct{})
	go func() {
		agent.Run(stopCh)
		close(done)
	}()
	<-sigCh
	fmt.Println("收到退出信号，正在停止…")
	close(stopCh)
	<-done
	return 0
}

// doInfo prints the registration info and hbbr liveness.
func doInfo(dir string) int {
	st, err := loadAgentState(dir)
	if err != nil {
		if err == errNotRegistered {
			fmt.Println("未注册：请先运行 relay-agent register 注册本节点。")
		} else {
			fmt.Println("错误：", err)
		}
		return 1
	}
	fmt.Printf("服务端：%s\n", st.Server)
	fmt.Printf("节点 id：%d\n", st.NodeID)
	fmt.Printf("节点名：%s\n", st.Name)
	if st.Address != "" {
		fmt.Printf("公网地址：%s\n", st.Address)
	}
	fmt.Printf("已应用配置版本：%d\n", st.AppliedVersion)
	env, err := loadRelayEnv(dir)
	if err != nil {
		fmt.Println("relay.env 读取失败：", err)
		return 1
	}
	port := env.EffectivePort()
	fmt.Printf("中继端口：%d\n", port)
	sessions, total, err := querySessions(port)
	if err != nil {
		fmt.Println("hbbr：未运行或控制台不通")
		return 0
	}
	fmt.Printf("hbbr：运行中，会话数 %d，总吞吐 %d kb/s\n", sessions, total)
	return 0
}
