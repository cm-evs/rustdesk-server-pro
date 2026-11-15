package main

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"
)

func hbbrBinaryName() string {
	if runtime.GOOS == "windows" {
		return "hbbr.exe"
	}
	return "hbbr"
}

func hbbrPath(dir string) string {
	return filepath.Join(dir, hbbrBinaryName())
}

// probeHbbrVersion runs `hbbr --version` once (5s timeout) and returns
// its first output line, or "unknown" on any failure.
func probeHbbrVersion(dir string) string {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	out, err := exec.CommandContext(ctx, hbbrPath(dir), "--version").Output()
	if err != nil {
		return "unknown"
	}
	first := strings.TrimSpace(strings.SplitN(string(out), "\n", 2)[0])
	if first == "" {
		return "unknown"
	}
	return first
}

// supervisor starts hbbr and keeps it alive: an unexpected exit is
// logged and retried after 3s, a start failure after 10s. A restart
// requested via RequestRestart kills the current child and starts a
// fresh one (possibly on a new port) without the backoff delay.
type supervisor struct {
	dir       string
	log       *logger
	mu        sync.Mutex
	cmd       *exec.Cmd
	pid       int
	startedAt int64
	port      int
	limits    map[string]string
	version   string

	stopCh      chan struct{}
	stopOnce    sync.Once
	restartCh   chan struct{}
	restartFlag bool
	wg          sync.WaitGroup
}

func newSupervisor(dir string, log *logger, port int, limits map[string]string) *supervisor {
	return &supervisor{
		dir: dir, log: log, port: port, limits: limits,
		stopCh:    make(chan struct{}),
		restartCh: make(chan struct{}, 1),
	}
}

func (s *supervisor) Start() {
	s.version = probeHbbrVersion(s.dir)
	if s.version == "unknown" {
		s.log.Logf("警告：无法获取 hbbr 版本（hbbr --version 失败），上报为 unknown")
	} else {
		s.log.Logf("hbbr 版本：%s", s.version)
	}
	s.wg.Add(1)
	go s.runLoop()
}

func (s *supervisor) Stop() {
	s.stopOnce.Do(func() { close(s.stopCh) })
	s.mu.Lock()
	cmd := s.cmd
	s.mu.Unlock()
	if cmd != nil {
		stopProcess(cmd)
	}
	s.wg.Wait()
}

func (s *supervisor) isStopped() bool {
	select {
	case <-s.stopCh:
		return true
	default:
		return false
	}
}

func (s *supervisor) Running() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.cmd != nil
}

func (s *supervisor) Pid() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.pid
}

func (s *supervisor) StartedAt() int64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.startedAt
}

func (s *supervisor) Port() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.port
}

func (s *supervisor) Version() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.version
}

func (s *supervisor) Limits() map[string]string {
	s.mu.Lock()
	defer s.mu.Unlock()
	cp := make(map[string]string, len(s.limits))
	for k, v := range s.limits {
		cp[k] = v
	}
	return cp
}

// SetLimits updates the limit env overrides applied at the next
// (re)start; it does not restart a running hbbr.
func (s *supervisor) SetLimits(limits map[string]string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.limits = limits
}

// RequestRestart kills the running hbbr (if any) and restarts it with
// the given port and limit overrides. If hbbr is currently in a
// start-failure backoff, the sleep is cut short.
func (s *supervisor) RequestRestart(port int, limits map[string]string) {
	s.mu.Lock()
	s.port = port
	s.limits = limits
	s.restartFlag = true
	cmd := s.cmd
	s.mu.Unlock()
	select {
	case s.restartCh <- struct{}{}:
	default:
	}
	if cmd != nil {
		s.log.Logf("收到重启请求，正在重启 hbbr（端口 %d）", port)
		stopProcess(cmd)
	}
}

func (s *supervisor) snapshot() (int, map[string]string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	cp := make(map[string]string, len(s.limits))
	for k, v := range s.limits {
		cp[k] = v
	}
	return s.port, cp
}

func (s *supervisor) takeRestartFlag() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	f := s.restartFlag
	s.restartFlag = false
	return f
}

// sleepInterruptible sleeps d unless the supervisor is stopped
// (returns false) or a restart was requested (returns true early).
func (s *supervisor) sleepInterruptible(d time.Duration) bool {
	select {
	case <-s.stopCh:
		return false
	case <-s.restartCh:
		return true
	case <-time.After(d):
		return true
	}
}

func (s *supervisor) runLoop() {
	defer s.wg.Done()
	for {
		if s.isStopped() {
			return
		}
		// Drain a stale restart wake-up so it cannot cut a
		// legitimate backoff short.
		select {
		case <-s.restartCh:
		default:
		}
		port, limits := s.snapshot()
		// hbbr only enforces the server key when started with -k:
		// without it the relay accepts any licence key. -k - makes
		// hbbr load the delivered id_ed25519 keypair (same as
		// hbbs's default). Never start hbbr without the key file:
		// with the file missing, -k - would silently generate a
		// fresh keypair and the relay would reject legit clients.
		if _, err := os.Stat(filepath.Join(s.dir, "id_ed25519")); err != nil {
			s.log.Logf("错误：id_ed25519 不存在，拒绝启动无密钥中继（10 秒后重试）")
			if !s.sleepInterruptible(10 * time.Second) {
				return
			}
			continue
		}
		cmd := exec.Command(hbbrPath(s.dir), "-p", strconv.Itoa(port), "-k", "-")
		cmd.Dir = s.dir
		env := os.Environ()
		for k, v := range limits {
			env = append(env, k+"="+v)
		}
		cmd.Env = env
		var out *os.File
		if err := os.MkdirAll(filepath.Join(s.dir, "logs"), 0o755); err == nil {
			// hbbr's own logger already writes logs/hbbr.log;
			// stdout/stderr go to a separate file so lines are
			// not duplicated (and panics are still captured).
			if f, err := os.OpenFile(filepath.Join(s.dir, "logs", "hbbr.out"),
				os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644); err == nil {
				out = f
				cmd.Stdout = f
				cmd.Stderr = f
			} else {
				s.log.Logf("警告：无法打开 logs/hbbr.out：%v", err)
			}
		}
		if err := cmd.Start(); err != nil {
			s.log.Logf("hbbr 启动失败：%v（10 秒后重试）", err)
			if out != nil {
				out.Close()
			}
			if !s.sleepInterruptible(10 * time.Second) {
				return
			}
			continue
		}
		s.mu.Lock()
		s.cmd = cmd
		s.pid = cmd.Process.Pid
		s.startedAt = time.Now().Unix()
		s.mu.Unlock()
		s.log.Logf("hbbr 已启动 pid=%d 端口=%d", cmd.Process.Pid, port)
		// A restart may have been requested while this child was
		// starting (RequestRestart saw no process to kill). Kill
		// it now so the next iteration picks up the new port.
		restartPending := false
		if s.takeRestartFlag() {
			restartPending = true
			s.log.Logf("hbbr 启动期间收到重启请求，立即重启")
			stopProcess(cmd)
		}
		waitErr := cmd.Wait()
		if out != nil {
			out.Close()
		}
		s.mu.Lock()
		s.cmd = nil
		s.pid = 0
		s.startedAt = 0
		s.mu.Unlock()
		if s.isStopped() {
			return
		}
		if restartPending || s.takeRestartFlag() {
			s.log.Logf("hbbr 已停止，正在按要求重启")
			continue
		}
		s.log.Logf("hbbr 意外退出：%v（3 秒后重启）", waitErr)
		if !s.sleepInterruptible(3 * time.Second) {
			return
		}
	}
}
