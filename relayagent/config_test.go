package main

import (
	"os"
	"runtime"
	"strings"
	"testing"
)

func TestParseRelayEnv(t *testing.T) {
	data := `# 注释行
HBBR_PORT=21119

LIMIT_SPEED="64"
TOTAL_BANDWIDTH='512.5'
SINGLE_BANDWIDTH=
UNKNOWN_KEY=123
MALFORMED LINE
HBBR_PORT=notaport
`
	env := parseRelayEnv(data)
	if env.Port != 21119 {
		t.Fatalf("port = %d, want 21119", env.Port)
	}
	if env.LimitSpeed == nil || *env.LimitSpeed != 64 {
		t.Fatalf("limit_speed = %v, want 64", env.LimitSpeed)
	}
	if env.TotalBandwidth == nil || *env.TotalBandwidth != 512.5 {
		t.Fatalf("total_bandwidth = %v, want 512.5", env.TotalBandwidth)
	}
	if env.SingleBandwidth != nil {
		t.Fatalf("single_bandwidth = %v, want nil (empty value ignored)", env.SingleBandwidth)
	}
}

func TestRelayEnvWriteRoundTrip(t *testing.T) {
	dir := t.TempDir()
	ls, sb := 64.0, 256.0
	in := RelayEnv{Port: 21119, LimitSpeed: &ls, SingleBandwidth: &sb}
	if err := writeRelayEnv(dir, in); err != nil {
		t.Fatal(err)
	}
	// null 的限速项不写这一行。
	rawBytes, err := os.ReadFile(dir + "/relay.env")
	raw := string(rawBytes)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(raw, "TOTAL_BANDWIDTH") {
		t.Fatalf("null limit was written:\n%s", raw)
	}
	out, err := loadRelayEnv(dir)
	if err != nil {
		t.Fatal(err)
	}
	if out.Port != 21119 || out.LimitSpeed == nil || *out.LimitSpeed != 64 ||
		out.TotalBandwidth != nil || out.SingleBandwidth == nil || *out.SingleBandwidth != 256 {
		t.Fatalf("round-trip mismatch: %+v", out)
	}
}

func TestLoadRelayEnvMissing(t *testing.T) {
	env, err := loadRelayEnv(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if env.EffectivePort() != DefaultPort {
		t.Fatalf("missing file port = %d, want %d", env.EffectivePort(), DefaultPort)
	}
}

func TestRelayConfigEffectiveLimits(t *testing.T) {
	ls, tb, sb := RelayConfig{}.EffectiveLimits()
	if ls != 32 || tb != 1024 || sb != 128 {
		t.Fatalf("defaults = %v/%v/%v, want 32/1024/128", ls, tb, sb)
	}
	v := 64.0
	ls, tb, sb = RelayConfig{LimitSpeed: &v}.EffectiveLimits()
	if ls != 64 || tb != 1024 || sb != 128 {
		t.Fatalf("partial = %v/%v/%v, want 64/1024/128", ls, tb, sb)
	}
}

func TestNeedsRestart(t *testing.T) {
	if NeedsRestart(21117, RelayConfig{Port: 21117}) {
		t.Fatal("same port should not restart")
	}
	if !NeedsRestart(21117, RelayConfig{Port: 21119}) {
		t.Fatal("port change should restart")
	}
	if NeedsRestart(21117, RelayConfig{}) {
		t.Fatal("unset port defaults to 21117, should not restart")
	}
}

func TestAgentStateRoundTrip(t *testing.T) {
	dir := t.TempDir()
	in := &AgentState{
		Server: "http://127.0.0.1:21114", NodeID: 12,
		LinkKey: strings.Repeat("ab", 32), Name: "node1",
		Address: "1.2.3.4", AppliedVersion: 3,
	}
	if err := saveAgentState(dir, in); err != nil {
		t.Fatal(err)
	}
	if runtime.GOOS != "windows" {
		fi, err := os.Stat(dir + "/agent.json")
		if err != nil {
			t.Fatal(err)
		}
		if fi.Mode().Perm() != 0o600 {
			t.Fatalf("agent.json perm = %o, want 0600", fi.Mode().Perm())
		}
	}
	out, err := loadAgentState(dir)
	if err != nil {
		t.Fatal(err)
	}
	if *out != *in {
		t.Fatalf("round-trip mismatch: %+v vs %+v", out, in)
	}
}

func TestLoadAgentStateMissing(t *testing.T) {
	if _, err := loadAgentState(t.TempDir()); err != errNotRegistered {
		t.Fatalf("err = %v, want errNotRegistered", err)
	}
}
