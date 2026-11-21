// relay-agent — resident agent for a rustdesk-server-pro relay node.
//
// hbbr (the relay server) never talks to hbbs (the ID server), so a
// relay host runs this agent next to hbbr: it registers the node with
// hbbs, heartbeats every 20s, reports hbbr status (including the
// node's own public IP, detected via an external lookup service),
// applies configuration pushed by the server (listen port + speed
// limits), executes restart commands, and supervises the local hbbr
// process (start/stop/restart).
//
// Usage:
//
//	relay-agent register --link <注册链接> [--name <名>] [--address <公网地址>]
//	relay-agent register --server <url> --token <rrt_...> --key <64hex> [--name <名>] [--address <公网地址>]
//	relay-agent run        前台运行（Unix 下无参数时默认）
//	relay-agent info       打印注册信息与 hbbr 状态
//	relay-agent version    打印代理版本
//	relay-agent install / uninstall / start / stop / restart / status  （仅 Windows，服务管理）
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

const agentVersion = "1.0.0"

func exeDir() string {
	exe, err := os.Executable()
	if err != nil {
		return "."
	}
	return filepath.Dir(exe)
}

const usage = `relay-agent — RustDesk 中继节点代理 v1.0.0

用法:
  relay-agent register --link <注册链接> [--name <名>] [--address <公网地址>]
  relay-agent register --server <url> --token <rrt_...> --key <64hex> [--name <名>] [--address <公网地址>]
  relay-agent run        前台运行：监督 hbbr 并向服务端心跳
  relay-agent info       打印注册信息与 hbbr 状态
  relay-agent version    打印代理版本
  relay-agent install    注册为 Windows 服务（仅 Windows，需管理员）
  relay-agent uninstall  停止并卸载服务（仅 Windows）
  relay-agent start      启动服务（仅 Windows）
  relay-agent stop       停止服务（仅 Windows）
  relay-agent restart    重启服务（仅 Windows）
  relay-agent status     查看服务状态（仅 Windows）

  Unix 下无参数时等同于 run；Windows 下无参数时以服务方式运行。
`

func main() {
	if len(os.Args) < 2 {
		os.Exit(runDefault())
	}
	switch cmd := strings.ToLower(os.Args[1]); cmd {
	case "register":
		os.Exit(doRegister(exeDir(), os.Args[2:]))
	case "run":
		os.Exit(runForeground())
	case "info":
		os.Exit(doInfo(exeDir()))
	case "version":
		fmt.Println("relay-agent " + agentVersion)
	case "-h", "--help", "help":
		fmt.Print(usage)
	case "install", "uninstall", "start", "stop", "restart", "status":
		os.Exit(doServiceCommand(cmd))
	default:
		fmt.Printf("未知命令：%s\n\n", os.Args[1])
		fmt.Print(usage)
		os.Exit(1)
	}
}
