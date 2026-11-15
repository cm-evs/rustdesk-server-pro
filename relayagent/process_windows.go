//go:build windows

package main

import "os/exec"

// stopProcess kills the child directly; Windows console programs get
// no graceful SIGTERM equivalent through the process handle.
func stopProcess(cmd *exec.Cmd) {
	if cmd == nil || cmd.Process == nil {
		return
	}
	_ = cmd.Process.Kill()
}
