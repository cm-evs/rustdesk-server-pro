package main

import (
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// logger appends timestamped lines to logs/relay-agent.log under dir,
// and optionally mirrors them to stdout (foreground mode).
type logger struct {
	mu     sync.Mutex
	dir    string
	stdout bool
}

func newLogger(dir string, stdout bool) *logger {
	return &logger{dir: dir, stdout: stdout}
}

func (l *logger) Logf(format string, a ...any) {
	line := time.Now().Format("2006-01-02 15:04:05") + " " + fmt.Sprintf(format, a...)
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.stdout {
		fmt.Println(line)
	}
	if err := os.MkdirAll(filepath.Join(l.dir, "logs"), 0o755); err != nil {
		return
	}
	f, err := os.OpenFile(filepath.Join(l.dir, "logs", "relay-agent.log"),
		os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return
	}
	defer f.Close()
	_, _ = f.WriteString(line + "\n")
}
