package main

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// hbbr built-in defaults (Mb/s) when neither a start argument nor an
// environment variable specifies a limit. A null limit in the server
// config means "use the hbbr default", never "unlimited" — there is
// no unlimited value.
const (
	DefaultPort            = 21117
	DefaultLimitSpeed      = 32
	DefaultTotalBandwidth  = 1024
	DefaultSingleBandwidth = 128
)

// RelayConfig is the server-pushed hbbr configuration. A nil limit
// means "use the hbbr default".
type RelayConfig struct {
	Port            int      `json:"port"`
	LimitSpeed      *float64 `json:"limit_speed"`
	TotalBandwidth  *float64 `json:"total_bandwidth"`
	SingleBandwidth *float64 `json:"single_bandwidth"`
}

// EffectiveLimits resolves nulls to the hbbr built-in defaults.
func (c RelayConfig) EffectiveLimits() (ls, tb, sb float64) {
	ls, tb, sb = DefaultLimitSpeed, DefaultTotalBandwidth, DefaultSingleBandwidth
	if c.LimitSpeed != nil {
		ls = *c.LimitSpeed
	}
	if c.TotalBandwidth != nil {
		tb = *c.TotalBandwidth
	}
	if c.SingleBandwidth != nil {
		sb = *c.SingleBandwidth
	}
	return ls, tb, sb
}

// EffectivePort returns the configured port, or the default when unset.
func (c RelayConfig) EffectivePort() int {
	if c.Port <= 0 {
		return DefaultPort
	}
	return c.Port
}

// NeedsRestart reports whether applying new requires an hbbr restart
// (only a port change does; limit changes apply live via the loopback
// console).
func NeedsRestart(oldPort int, new RelayConfig) bool {
	return new.EffectivePort() != oldPort
}

// ---------------------------------------------------------------------------
// agent.json — registration credential + sync state

// AgentState is persisted in agent.json next to the agent binary.
// The long-term credential is LinkKey: the 32-byte symmetric key from
// the registration link (64 lowercase hex chars). There is no
// node_token; the link key authenticates every heartbeat.
type AgentState struct {
	Server         string `json:"server"`
	NodeID         int    `json:"node_id"`
	LinkKey        string `json:"link_key"`
	Name           string `json:"name"`
	Address        string `json:"address"`
	AppliedVersion int    `json:"applied_version"`
}

var errNotRegistered = errors.New("未注册")

func agentStatePath(dir string) string { return filepath.Join(dir, "agent.json") }

func loadAgentState(dir string) (*AgentState, error) {
	data, err := os.ReadFile(agentStatePath(dir))
	if err != nil {
		if os.IsNotExist(err) {
			return nil, errNotRegistered
		}
		return nil, err
	}
	var st AgentState
	if err := json.Unmarshal(data, &st); err != nil {
		return nil, fmt.Errorf("agent.json 解析失败: %w", err)
	}
	if st.LinkKey == "" {
		return nil, errNotRegistered
	}
	return &st, nil
}

// saveAgentState writes agent.json atomically (temp file + rename)
// with 0600 permissions, so the node token is not world-readable on
// Unix.
func saveAgentState(dir string, st *AgentState) error {
	data, err := json.MarshalIndent(st, "", "  ")
	if err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, "agent.json.tmp-*")
	if err != nil {
		return err
	}
	tmpName := tmp.Name()
	if _, err := tmp.Write(append(data, '\n')); err != nil {
		tmp.Close()
		os.Remove(tmpName)
		return err
	}
	if err := tmp.Chmod(0o600); err != nil {
		tmp.Close()
		os.Remove(tmpName)
		return err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmpName)
		return err
	}
	return os.Rename(tmpName, agentStatePath(dir))
}

// ---------------------------------------------------------------------------
// relay.env — agent-managed hbbr configuration

// RelayEnv is the agent-managed hbbr configuration persisted in
// relay.env. A nil limit means "omit the line, let hbbr use its
// built-in default". Port 0 means unset (default 21117).
type RelayEnv struct {
	Port            int
	LimitSpeed      *float64
	TotalBandwidth  *float64
	SingleBandwidth *float64
}

// ToEnv converts a server config to its relay.env form.
func (c RelayConfig) ToEnv() RelayEnv {
	return RelayEnv{
		Port:            c.EffectivePort(),
		LimitSpeed:      c.LimitSpeed,
		TotalBandwidth:  c.TotalBandwidth,
		SingleBandwidth: c.SingleBandwidth,
	}
}

// EffectivePort returns the configured port, or the default when unset.
func (e RelayEnv) EffectivePort() int {
	if e.Port <= 0 {
		return DefaultPort
	}
	return e.Port
}

// EnvMap returns the limit overrides as environment variables (only
// non-nil limits are included).
func (e RelayEnv) EnvMap() map[string]string {
	m := make(map[string]string)
	if e.LimitSpeed != nil {
		m["LIMIT_SPEED"] = strconv.FormatFloat(*e.LimitSpeed, 'g', -1, 64)
	}
	if e.TotalBandwidth != nil {
		m["TOTAL_BANDWIDTH"] = strconv.FormatFloat(*e.TotalBandwidth, 'g', -1, 64)
	}
	if e.SingleBandwidth != nil {
		m["SINGLE_BANDWIDTH"] = strconv.FormatFloat(*e.SingleBandwidth, 'g', -1, 64)
	}
	return m
}

// parseRelayEnv parses KEY=value lines with the same rules as
// winservice's server.env: '#' comments and blank lines are skipped,
// empty values never override, one pair of surrounding quotes is
// stripped. Malformed lines and unparsable values are ignored.
func parseRelayEnv(data string) RelayEnv {
	env := RelayEnv{}
	sc := bufio.NewScanner(strings.NewReader(data))
	sc.Buffer(make([]byte, 64*1024), 64*1024)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		idx := strings.Index(line, "=")
		if idx <= 0 {
			continue
		}
		key := strings.TrimSpace(line[:idx])
		val := strings.TrimSpace(line[idx+1:])
		if val == "" {
			continue
		}
		if len(val) >= 2 {
			first, last := val[0], val[len(val)-1]
			if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
				val = val[1 : len(val)-1]
			}
		}
		switch key {
		case "HBBR_PORT":
			if p, err := strconv.Atoi(val); err == nil && p > 0 {
				env.Port = p
			}
		case "LIMIT_SPEED":
			if f, err := strconv.ParseFloat(val, 64); err == nil {
				env.LimitSpeed = &f
			}
		case "TOTAL_BANDWIDTH":
			if f, err := strconv.ParseFloat(val, 64); err == nil {
				env.TotalBandwidth = &f
			}
		case "SINGLE_BANDWIDTH":
			if f, err := strconv.ParseFloat(val, 64); err == nil {
				env.SingleBandwidth = &f
			}
		}
	}
	return env
}

// loadRelayEnv reads relay.env; a missing file means "defaults".
func loadRelayEnv(dir string) (RelayEnv, error) {
	data, err := os.ReadFile(filepath.Join(dir, "relay.env"))
	if err != nil {
		if os.IsNotExist(err) {
			return RelayEnv{}, nil
		}
		return RelayEnv{}, err
	}
	return parseRelayEnv(string(data)), nil
}

// writeRelayEnv persists the config. Nil limits are omitted so hbbr
// falls back to its built-in defaults.
func writeRelayEnv(dir string, env RelayEnv) error {
	var b strings.Builder
	fmt.Fprintf(&b, "HBBR_PORT=%d\n", env.EffectivePort())
	if env.LimitSpeed != nil {
		fmt.Fprintf(&b, "LIMIT_SPEED=%s\n", strconv.FormatFloat(*env.LimitSpeed, 'g', -1, 64))
	}
	if env.TotalBandwidth != nil {
		fmt.Fprintf(&b, "TOTAL_BANDWIDTH=%s\n", strconv.FormatFloat(*env.TotalBandwidth, 'g', -1, 64))
	}
	if env.SingleBandwidth != nil {
		fmt.Fprintf(&b, "SINGLE_BANDWIDTH=%s\n", strconv.FormatFloat(*env.SingleBandwidth, 'g', -1, 64))
	}
	return os.WriteFile(filepath.Join(dir, "relay.env"), []byte(b.String()), 0o644)
}
