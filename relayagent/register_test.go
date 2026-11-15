package main

import (
	"encoding/hex"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"runtime"
	"strings"
	"sync"
	"testing"
)

const (
	mockToken   = "rrt_" + "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
	mockKeyHex  = "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a"
	mockNonce   = "1234567890abcdef1234567890abcdef"
	mockPrivKey = "MOCK-PRIVATE-KEY-CONTENT\n"
	mockPubKey  = "MOCK-PUBLIC-KEY-CONTENT\n"
)

func mockLinkKey(t *testing.T) []byte {
	t.Helper()
	key, err := hex.DecodeString(mockKeyHex)
	if err != nil {
		t.Fatal(err)
	}
	return key
}

func TestParseRegisterLink(t *testing.T) {
	link := "http://27.45.254.164:21114/relay/register?token=" + mockToken + "&key=" + mockKeyHex
	server, token, key, err := parseRegisterLink(link)
	if err != nil {
		t.Fatal(err)
	}
	if server != "http://27.45.254.164:21114" {
		t.Fatalf("server = %q", server)
	}
	if token != mockToken {
		t.Fatalf("token = %q", token)
	}
	if hex.EncodeToString(key) != mockKeyHex {
		t.Fatalf("key = %x", key)
	}

	bad := map[string]string{
		"缺少 key":    "http://h:21114/relay/register?token=" + mockToken,
		"缺少 token":  "http://h:21114/relay/register?key=" + mockKeyHex,
		"key 太短":    "http://h:21114/relay/register?token=" + mockToken + "&key=aabb",
		"key 非 hex": "http://h:21114/relay/register?token=" + mockToken + "&key=" + strings.Repeat("zz", 32),
		"token 无前缀": "http://h:21114/relay/register?token=abc&key=" + mockKeyHex,
		"ftp 协议":    "ftp://h:21114/relay/register?token=" + mockToken + "&key=" + mockKeyHex,
		"无主机":       "http:///relay/register?token=" + mockToken + "&key=" + mockKeyHex,
		"空链接":       "",
	}
	for name, l := range bad {
		if _, _, _, err := parseRegisterLink(l); err == nil {
			t.Errorf("%s：非法链接被接受", name)
		}
	}
}

// mockRelayServer implements the frozen registration protocol:
// challenge issues a one-time nonce, register verifies the nonce
// (single-use) + proof and returns an encrypted bundle.
type mockRelayServer struct {
	t      *testing.T
	key    []byte
	mu     sync.Mutex
	nonce  string
	used   map[string]bool
	badKey bool // encrypt the bundle with a wrong key
	failCh bool // challenge returns ok=false
	regs   int
	challs int
}

func newMockRelayServer(t *testing.T) (*mockRelayServer, *httptest.Server) {
	m := &mockRelayServer{t: t, key: mockLinkKey(t), used: map[string]bool{}}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch r.URL.Path {
		case "/api/relay/challenge":
			m.mu.Lock()
			m.challs++
			failCh := m.failCh
			m.mu.Unlock()
			if failCh {
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "注册链接无效或已过期"})
				return
			}
			// A fresh nonce per challenge; the mock reuses one
			// fixed value but tracks single-use per value, so a
			// second challenge after a spent nonce issues a new one.
			m.mu.Lock()
			nonce := mockNonce
			if m.used[nonce] {
				nonce = "fedcba0987654321fedcba0987654321"
			}
			m.nonce = nonce
			m.mu.Unlock()
			_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "nonce": nonce})
		case "/api/relay/register":
			var req registerRequest
			if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
				t.Errorf("register 请求解析失败: %v", err)
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "bad request"})
				return
			}
			m.mu.Lock()
			defer m.mu.Unlock()
			m.regs++
			if req.TokenID != tokenID(mockToken) {
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "注册链接无效或已过期"})
				return
			}
			if req.Nonce != m.nonce || m.used[req.Nonce] {
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "质询已失效，请重试"})
				return
			}
			// Nonce is spent after one verification, success or not.
			m.used[req.Nonce] = true
			want := registerProof(m.key, req.TokenID, req.Nonce, req.Name, req.AgentVersion, req.Address)
			if req.Proof != want {
				_ = json.NewEncoder(w).Encode(map[string]any{"ok": false, "error": "验证失败"})
				return
			}
			encKey := m.key
			if m.badKey {
				encKey, _ = hex.DecodeString(strings.Repeat("99", 32))
			}
			plain, _ := json.Marshal(registrationBundle{
				NodeID: 12, PrivateKey: mockPrivKey, PublicKey: mockPubKey,
				Config: RelayConfig{Port: 21117}, ConfigVersion: 1,
			})
			b, err := encryptBundle(encKey, plain)
			if err != nil {
				t.Fatal(err)
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"ok": true, "bundle": b})
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	return m, srv
}

func TestRegisterChallengeResponseFlow(t *testing.T) {
	_, srv := newMockRelayServer(t)
	defer srv.Close()
	dir := t.TempDir()
	code := doRegister(dir, []string{"--server", srv.URL, "--token", mockToken, "--key", mockKeyHex, "--name", "node1", "--address", "1.2.3.4"})
	if code != 0 {
		t.Fatalf("doRegister exit = %d, want 0", code)
	}
	st, err := loadAgentState(dir)
	if err != nil {
		t.Fatal(err)
	}
	if st.NodeID != 12 || st.LinkKey != mockKeyHex || st.Name != "node1" || st.AppliedVersion != 1 {
		t.Fatalf("agent.json = %+v", st)
	}
	priv, err := os.ReadFile(dir + "/id_ed25519")
	if err != nil || string(priv) != mockPrivKey {
		t.Fatalf("id_ed25519 = %q, err %v", priv, err)
	}
	pub, err := os.ReadFile(dir + "/id_ed25519.pub")
	if err != nil || string(pub) != mockPubKey {
		t.Fatalf("id_ed25519.pub = %q, err %v", pub, err)
	}
	if runtime.GOOS != "windows" {
		if fi, _ := os.Stat(dir + "/id_ed25519"); fi.Mode().Perm() != 0o600 {
			t.Fatalf("id_ed25519 perm = %o, want 0600", fi.Mode().Perm())
		}
		if fi, _ := os.Stat(dir + "/id_ed25519.pub"); fi.Mode().Perm() != 0o644 {
			t.Fatalf("id_ed25519.pub perm = %o, want 0644", fi.Mode().Perm())
		}
	}
	env, err := loadRelayEnv(dir)
	if err != nil || env.Port != 21117 {
		t.Fatalf("relay.env = %+v, err %v", env, err)
	}
	// Second registration is refused (existing credentials).
	code = doRegister(dir, []string{"--server", srv.URL, "--token", mockToken, "--key", mockKeyHex})
	if code == 0 {
		t.Fatal("重复注册未被拒绝")
	}
}

func TestRegisterViaLink(t *testing.T) {
	_, srv := newMockRelayServer(t)
	defer srv.Close()
	link := srv.URL + "/relay/register?token=" + mockToken + "&key=" + mockKeyHex
	dir := t.TempDir()
	if code := doRegister(dir, []string{"--link", link, "--name", "via-link"}); code != 0 {
		t.Fatalf("doRegister --link exit = %d, want 0", code)
	}
	st, err := loadAgentState(dir)
	if err != nil {
		t.Fatal(err)
	}
	if st.Server != srv.URL || st.Name != "via-link" {
		t.Fatalf("agent.json = %+v", st)
	}
}

func TestRegisterKeyBackup(t *testing.T) {
	_, srv := newMockRelayServer(t)
	defer srv.Close()
	dir := t.TempDir()
	// Pre-existing different keys must be backed up, not lost.
	if err := os.WriteFile(dir+"/id_ed25519", []byte("OLD-PRIVATE\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dir+"/id_ed25519.pub", []byte("OLD-PUBLIC\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if code := doRegister(dir, []string{"--server", srv.URL, "--token", mockToken, "--key", mockKeyHex}); code != 0 {
		t.Fatalf("doRegister exit = %d, want 0", code)
	}
	bak, err := os.ReadFile(dir + "/id_ed25519.bak")
	if err != nil || string(bak) != "OLD-PRIVATE\n" {
		t.Fatalf("id_ed25519.bak = %q, err %v", bak, err)
	}
	bak, err = os.ReadFile(dir + "/id_ed25519.pub.bak")
	if err != nil || string(bak) != "OLD-PUBLIC\n" {
		t.Fatalf("id_ed25519.pub.bak = %q, err %v", bak, err)
	}
	priv, _ := os.ReadFile(dir + "/id_ed25519")
	if string(priv) != mockPrivKey {
		t.Fatalf("id_ed25519 = %q, want new key", priv)
	}
}

func TestRegisterDecryptFailure(t *testing.T) {
	m, srv := newMockRelayServer(t)
	defer srv.Close()
	m.badKey = true
	dir := t.TempDir()
	if code := doRegister(dir, []string{"--server", srv.URL, "--token", mockToken, "--key", mockKeyHex}); code == 0 {
		t.Fatal("解密失败的注册未被拒绝")
	}
	if _, err := os.Stat(dir + "/agent.json"); !os.IsNotExist(err) {
		t.Fatal("解密失败后仍写了 agent.json")
	}
	if _, err := os.Stat(dir + "/id_ed25519"); !os.IsNotExist(err) {
		t.Fatal("解密失败后仍写了 id_ed25519")
	}
}

func TestRegisterChallengeAppError(t *testing.T) {
	m, srv := newMockRelayServer(t)
	defer srv.Close()
	m.failCh = true
	dir := t.TempDir()
	if code := doRegister(dir, []string{"--server", srv.URL, "--token", mockToken, "--key", mockKeyHex}); code == 0 {
		t.Fatal("质询失败的注册未被拒绝")
	}
	// App errors are not retried: exactly one challenge attempt.
	if m.challs != 1 {
		t.Fatalf("challenge attempts = %d, want 1", m.challs)
	}
}

func TestRegisterMissingKeyFlag(t *testing.T) {
	dir := t.TempDir()
	if code := doRegister(dir, []string{"--server", "http://127.0.0.1:9", "--token", mockToken}); code == 0 {
		t.Fatal("缺少 --key 的注册未被拒绝")
	}
}
