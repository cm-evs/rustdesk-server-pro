//go:build !windows

package main

import "fmt"

// runDefault is the no-argument behaviour on Unix: run in the
// foreground, like `run`.
func runDefault() int {
	return runForeground()
}

// doServiceCommand stubs the Windows-only service subcommands.
func doServiceCommand(cmd string) int {
	fmt.Printf("命令 %s 仅 Windows 可用。\n", cmd)
	return 1
}
