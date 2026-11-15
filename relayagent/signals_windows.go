//go:build windows

package main

import "os"

// shutdownSignals are the signals that stop a foreground run.
func shutdownSignals() []os.Signal {
	return []os.Signal{os.Interrupt}
}
