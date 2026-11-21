//go:build !windows

package main

import (
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

// writeFakeHbbr installs a stub hbbr: --version prints a version line,
// anything else sleeps (standing in for a long-running relay process).
func writeFakeHbbr(t *testing.T, dir string) {
	t.Helper()
	script := "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"hbbr 9.9.9-test\"; exit 0; fi\nexec sleep 300\n"
	if err := os.WriteFile(filepath.Join(dir, "hbbr"), []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
}

func writeTestRegistration(t *testing.T, dir, server string) {
	t.Helper()
	st := &AgentState{
		Server: server, NodeID: 12, LinkKey: mockKeyHex,
		Name: "test-node", Address: "", AppliedVersion: 1,
	}
	if err := saveAgentState(dir, st); err != nil {
		t.Fatal(err)
	}
	if err := writeRelayEnv(dir, RelayEnv{Port: 21117}); err != nil {
		t.Fatal(err)
	}
	// A registered node always has key files; the supervisor refuses
	// to start hbbr without id_ed25519 (fail-closed, no open relay).
	// The fake hbbr never reads it, so a dummy file suffices here.
	if err := os.WriteFile(filepath.Join(dir, "id_ed25519"),
		[]byte("dummy-test-key"), 0o600); err != nil {
		t.Fatal(err)
	}
}

func waitFor(t *testing.T, timeout time.Duration, cond func() bool, what string) {
	t.Helper()
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if cond() {
			return
		}
		time.Sleep(25 * time.Millisecond)
	}
	t.Fatalf("超时等待：%s", what)
}

// TestApplyConfigDecisions verifies the apply decision matrix against
// a live (fake) hbbr: limits-only changes do not restart, a port
// change does.
func TestApplyConfigDecisions(t *testing.T) {
	dir := t.TempDir()
	writeFakeHbbr(t, dir)
	writeTestRegistration(t, dir, "http://127.0.0.1:9")
	agent, err := buildAgent(dir, false)
	if err != nil {
		t.Fatal(err)
	}
	agent.sup.Start()
	defer agent.sup.Stop()
	waitFor(t, 5*time.Second, agent.sup.Running, "hbbr 启动")
	pid1 := agent.sup.Pid()
	if pid1 == 0 {
		t.Fatal("hbbr pid 未记录")
	}

	// Limits only: no restart, version advances, relay.env updated.
	ls := 64.0
	agent.applyConfig(RelayConfig{Port: 21117, LimitSpeed: &ls}, 2)
	time.Sleep(300 * time.Millisecond)
	if agent.sup.Pid() != pid1 {
		t.Error("仅限速变化不应重启 hbbr")
	}
	if agent.state.AppliedVersion != 2 {
		t.Fatalf("applied_version = %d, want 2", agent.state.AppliedVersion)
	}
	env, _ := loadRelayEnv(dir)
	if env.LimitSpeed == nil || *env.LimitSpeed != 64 {
		t.Fatalf("relay.env LIMIT_SPEED = %v, want 64", env.LimitSpeed)
	}
	if env.TotalBandwidth != nil {
		t.Fatalf("relay.env 不应写入 null 的 TOTAL_BANDWIDTH")
	}

	// Port change: restart with the new port.
	agent.applyConfig(RelayConfig{Port: 21119}, 3)
	waitFor(t, 5*time.Second, func() bool {
		return agent.sup.Running() && agent.sup.Pid() != pid1
	}, "端口变化后 hbbr 重启")
	if agent.sup.Port() != 21119 {
		t.Fatalf("监督端口 = %d, want 21119", agent.sup.Port())
	}
	if agent.state.AppliedVersion != 3 {
		t.Fatalf("applied_version = %d, want 3", agent.state.AppliedVersion)
	}
}

// fullMockServer implements challenge + register + heartbeat per the
// frozen encrypted protocol, verifying proofs and signatures.
type fullMockServer struct {
	t     *testing.T
	key   []byte
	mu    sync.Mutex
	nonce string
	used  map[string]bool
	hb    int
	sigOK int
	// lastPublicIP is the public_ip of the most recent heartbeat.
	lastPublicIP string
	sawAck       bool
	revoke       bool // revoke once the restart ack is seen
	badHB        bool // encrypt heartbeat bundles with a wrong key
}

func newFullMockServer(t *testing.T) (*fullMockServer, *httptest.Server) {
	m := &fullMockServer{t: t, key: mockLinkKey(t), used: map[string]bool{}}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/api/relay/challenge":
			m.mu.Lock()
			m.nonce = mockNonce
			m.mu.Unlock()
			_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "nonce": mockNonce})
		case "/api/relay/register":
			var req registerRequest
			_ = json.NewDecoder(r.Body).Decode(&req)
			m.mu.Lock()
			defer m.mu.Unlock()
			if req.TokenID != tokenID(mockToken) || req.Nonce != m.nonce || m.used[req.Nonce] {
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "注册链接无效或已过期"})
				return
			}
			m.used[req.Nonce] = true
			if req.Proof != registerProof(m.key, req.TokenID, req.Nonce, req.Name, req.AgentVersion, req.Address) {
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "验证失败"})
				return
			}
			plain, _ := json.Marshal(registrationBundle{
				NodeID: 12, PrivateKey: mockPrivKey, PublicKey: mockPubKey,
				Config: RelayConfig{Port: 21117}, ConfigVersion: 1,
			})
			b, err := encryptBundle(m.key, plain)
			if err != nil {
				t.Fatal(err)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "bundle": b})
		case "/api/relay/heartbeat":
			var req heartbeatRequest
			_ = json.NewDecoder(r.Body).Decode(&req)
			m.mu.Lock()
			defer m.mu.Unlock()
			m.hb++
			if heartbeatSig(m.key, req.NodeID, req.TS, req.Nonce, req.Bundle.Nonce, req.Bundle.Data) == req.Sig {
				m.sigOK++
			} else {
				t.Errorf("心跳签名验证失败（第 %d 次）", m.hb)
			}
			plain, err := decryptBundle(m.key, req.Bundle)
			if err != nil {
				t.Errorf("心跳 bundle 解密失败：%v", err)
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "decrypt failed"})
				return
			}
			var pt heartbeatPlaintext
			if err := json.Unmarshal(plain, &pt); err != nil {
				t.Errorf("心跳明文解析失败：%v", err)
				return
			}
			m.lastPublicIP = pt.Status.PublicIP
			if pt.CommandAck == "restart" {
				m.sawAck = true
			}
			if m.revoke && m.sawAck {
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "节点已删除", "revoked": true})
				return
			}
			if m.hb == 1 {
				ls := 64.0
				upd, _ := json.Marshal(heartbeatUpdate{
					Config:        &RelayConfig{Port: 21119, LimitSpeed: &ls},
					ConfigVersion: 2, Command: "restart",
				})
				encKey := m.key
				if m.badHB {
					encKey, _ = hex.DecodeString(strings.Repeat("99", 32))
				}
				b, err := encryptBundle(encKey, upd)
				if err != nil {
					t.Fatal(err)
				}
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "bundle": b})
				return
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "bundle": nil})
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	return m, srv
}

func (m *fullMockServer) counts() (hb, sigOK int, sawAck bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.hb, m.sigOK, m.sawAck
}

func (m *fullMockServer) publicIP() string {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.lastPublicIP
}

// TestFullFlow walks the whole lifecycle against the mock server:
// register → decrypt + write keys → heartbeat → decrypt + apply
// config → restart ack → revoked stops heartbeats.
func TestFullFlow(t *testing.T) {
	// No public-IP lookup in this test: the agent must not touch
	// the real lookup services.
	t.Setenv(publicIPEnvOverride, "")
	m, srv := newFullMockServer(t)
	defer srv.Close()
	m.revoke = true
	dir := t.TempDir()
	writeFakeHbbr(t, dir)

	if code := doRegister(dir, []string{"--server", srv.URL, "--token", mockToken, "--key", mockKeyHex, "--name", "node1"}); code != 0 {
		t.Fatalf("doRegister exit = %d, want 0", code)
	}
	if _, err := os.Stat(dir + "/id_ed25519"); err != nil {
		t.Fatalf("id_ed25519 未落盘: %v", err)
	}

	agent, err := buildAgent(dir, false)
	if err != nil {
		t.Fatal(err)
	}
	agent.interval = 100 * time.Millisecond
	stopCh := make(chan struct{})
	done := make(chan struct{})
	go func() { agent.Run(stopCh); close(done) }()
	defer func() { close(stopCh); <-done }()

	// Config applied (version 2) and restart ack delivered.
	waitFor(t, 10*time.Second, func() bool {
		st, err := loadAgentState(dir)
		if err != nil {
			return false
		}
		_, _, sawAck := m.counts()
		return st.AppliedVersion == 2 && sawAck
	}, "配置应用 + restart ack")
	env, _ := loadRelayEnv(dir)
	if env.Port != 21119 {
		t.Fatalf("relay.env 端口 = %d, want 21119", env.Port)
	}
	if agent.sup.Port() != 21119 {
		t.Fatalf("监督端口 = %d, want 21119", agent.sup.Port())
	}
	if !agent.sup.Running() {
		t.Fatal("应用配置后 hbbr 应在运行")
	}

	// Revoked: heartbeats stop, supervision continues.
	waitFor(t, 10*time.Second, agent.Revoked, "吊销后停止心跳")
	hbBefore, sigOK, _ := m.counts()
	if sigOK != hbBefore {
		t.Fatalf("签名验证通过 %d/%d 次心跳", sigOK, hbBefore)
	}
	time.Sleep(600 * time.Millisecond)
	hbAfter, _, _ := m.counts()
	if hbAfter != hbBefore {
		t.Fatalf("吊销后仍在心跳：%d -> %d", hbBefore, hbAfter)
	}
	if !agent.sup.Running() {
		t.Fatal("吊销后 hbbr 应继续受监督运行")
	}
}

// TestHeartbeatDecryptFailureIgnored: a heartbeat bundle the agent
// cannot decrypt is logged and skipped — no apply, no version bump,
// heartbeats continue.
func TestHeartbeatDecryptFailureIgnored(t *testing.T) {
	// No public-IP lookup in this test: the agent must not touch
	// the real lookup services.
	t.Setenv(publicIPEnvOverride, "")
	m, srv := newFullMockServer(t)
	defer srv.Close()
	m.badHB = true
	dir := t.TempDir()
	writeFakeHbbr(t, dir)
	writeTestRegistration(t, dir, srv.URL)
	agent, err := buildAgent(dir, false)
	if err != nil {
		t.Fatal(err)
	}
	agent.interval = 100 * time.Millisecond
	stopCh := make(chan struct{})
	done := make(chan struct{})
	go func() { agent.Run(stopCh); close(done) }()
	defer func() { close(stopCh); <-done }()

	waitFor(t, 10*time.Second, func() bool {
		hb, _, _ := m.counts()
		return hb >= 3
	}, "心跳持续进行")
	st, err := loadAgentState(dir)
	if err != nil {
		t.Fatal(err)
	}
	if st.AppliedVersion != 1 {
		t.Fatalf("解密失败后 applied_version = %d, want 1", st.AppliedVersion)
	}
	if agent.Revoked() {
		t.Fatal("解密失败不应触发吊销")
	}
}

// TestPublicIPReportedInHeartbeat: with the lookup endpoint pointed
// at a local stub, the agent detects its public IP at startup and
// reports it in the heartbeat status from the first heartbeat on.
func TestPublicIPReportedInHeartbeat(t *testing.T) {
	stub := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("203.0.113.7\n"))
	}))
	defer stub.Close()
	t.Setenv(publicIPEnvOverride, stub.URL)

	m, srv := newFullMockServer(t)
	defer srv.Close()
	dir := t.TempDir()
	writeFakeHbbr(t, dir)
	writeTestRegistration(t, dir, srv.URL)
	agent, err := buildAgent(dir, false)
	if err != nil {
		t.Fatal(err)
	}
	agent.interval = 100 * time.Millisecond
	stopCh := make(chan struct{})
	done := make(chan struct{})
	go func() { agent.Run(stopCh); close(done) }()
	defer func() { close(stopCh); <-done }()

	waitFor(t, 10*time.Second, func() bool {
		return m.publicIP() == "203.0.113.7"
	}, "心跳上报公网 IP")
}

// TestPublicIPNotReportedWhenDisabled: with detection disabled the
// heartbeat carries no public_ip — the same wire format an old
// agent sends, so the server keeps whatever it stored before.
func TestPublicIPNotReportedWhenDisabled(t *testing.T) {
	t.Setenv(publicIPEnvOverride, "")
	m, srv := newFullMockServer(t)
	defer srv.Close()
	dir := t.TempDir()
	writeFakeHbbr(t, dir)
	writeTestRegistration(t, dir, srv.URL)
	agent, err := buildAgent(dir, false)
	if err != nil {
		t.Fatal(err)
	}
	agent.interval = 100 * time.Millisecond
	stopCh := make(chan struct{})
	done := make(chan struct{})
	go func() { agent.Run(stopCh); close(done) }()
	defer func() { close(stopCh); <-done }()

	waitFor(t, 10*time.Second, func() bool {
		hb, _, _ := m.counts()
		return hb >= 2
	}, "心跳进行")
	if got := m.publicIP(); got != "" {
		t.Fatalf("public_ip = %q, want empty when detection is disabled", got)
	}
}
