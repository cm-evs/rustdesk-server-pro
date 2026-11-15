//go:build !windows

package main

import (
	"os/exec"
	"syscall"
	"time"
)

// stopProcess sends SIGTERM and waits up to 5s for the process to
// exit, then SIGKILLs it. The process is reaped by the supervisor's
// Wait call, not here.
func stopProcess(cmd *exec.Cmd) {
	if cmd == nil || cmd.Process == nil {
		return
	}
	_ = cmd.Process.Signal(syscall.SIGTERM)
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		// Signal 0 probes liveness; it fails once the process is
		// gone (and reaped by the supervisor's Wait).
		if err := cmd.Process.Signal(syscall.Signal(0)); err != nil {
			return
		}
		time.Sleep(100 * time.Millisecond)
	}
	_ = cmd.Process.Kill()
}
