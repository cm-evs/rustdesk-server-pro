//go:build !windows

package main

import (
	"os"
	"syscall"
)

// shutdownSignals are the signals that stop a foreground run.
func shutdownSignals() []os.Signal {
	return []os.Signal{os.Interrupt, syscall.SIGTERM}
}
