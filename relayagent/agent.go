package main

import (
	"encoding/json"
	"fmt"
	"math"
	"sync"
	"time"
)

const defaultHeartbeatInterval = 20 * time.Second

// Agent ties the supervisor and the hbbs API together: it heartbeats
// every interval (encrypted + signed), applies pushed configuration,
// and executes restart commands from the server.
type Agent struct {
	dir      string
	log      *logger
	state    *AgentState
	linkKey  []byte
	api      *apiClient
	sup      *supervisor
	pubIP    *publicIPDetector
	interval time.Duration

	mu         sync.Mutex
	pendingAck string
	revoked    bool
}

// buildAgent loads agent.json + relay.env and wires the supervisor.
// stdout controls whether log lines also mirror to stdout.
func buildAgent(dir string, stdout bool) (*Agent, error) {
	log := newLogger(dir, stdout)
	state, err := loadAgentState(dir)
	if err != nil {
		if err == errNotRegistered {
			return nil, fmt.Errorf("未注册：请先运行 relay-agent register 注册本节点")
		}
		return nil, err
	}
	linkKey, err := parseLinkKey(state.LinkKey)
	if err != nil {
		return nil, fmt.Errorf("agent.json 里的 link_key 无效：%w", err)
	}
	env, err := loadRelayEnv(dir)
	if err != nil {
		return nil, fmt.Errorf("relay.env 读取失败: %w", err)
	}
	sup := newSupervisor(dir, log, env.EffectivePort(), env.EnvMap())
	return &Agent{
		dir: dir, log: log, state: state, linkKey: linkKey,
		api:      newAPIClient(state.Server),
		sup:      sup,
		pubIP:    newPublicIPDetector(),
		interval: defaultHeartbeatInterval,
	}, nil
}

// Revoked reports whether the server has revoked this node.
func (a *Agent) Revoked() bool {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.revoked
}

func (a *Agent) setRevoked() {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.revoked = true
}

// Run starts hbbr supervision, heartbeats immediately and then every
// interval, until stopCh is closed. On revocation the heartbeat loop
// stops but hbbr supervision continues.
func (a *Agent) Run(stopCh <-chan struct{}) {
	a.log.Logf("relay-agent %s 启动，节点 %d（%s），服务端 %s",
		agentVersion, a.state.NodeID, a.state.Name, a.state.Server)
	a.sup.Start()
	// Detect the public IP before the first heartbeat so the
	// console shows it from the start; re-detects run hourly in
	// the background. Each lookup attempt has a short timeout, so
	// a dead service delays startup by seconds at most.
	a.pubIP.refresh(a.log)
	go a.pubIP.recheckLoop(stopCh, a.log)
	a.doHeartbeat()
	if a.Revoked() {
		a.waitStop(stopCh)
		return
	}
	ticker := time.NewTicker(a.interval)
	defer ticker.Stop()
	for {
		select {
		case <-stopCh:
			a.shutdown()
			return
		case <-ticker.C:
			a.doHeartbeat()
			if a.Revoked() {
				a.waitStop(stopCh)
				return
			}
		}
	}
}

func (a *Agent) shutdown() {
	a.log.Logf("正在停止 hbbr…")
	a.sup.Stop()
	a.log.Logf("relay-agent 已停止")
}

// waitStop blocks until the agent is stopped; the relay keeps running
// under supervision in the meantime (used after revocation).
func (a *Agent) waitStop(stopCh <-chan struct{}) {
	<-stopCh
	a.shutdown()
}

func (a *Agent) collectStatus() heartbeatStatus {
	st := heartbeatStatus{
		HbbrVersion:  a.sup.Version(),
		AgentVersion: agentVersion,
		ListenPort:   a.sup.Port(),
	}
	if a.pubIP != nil {
		st.PublicIP = a.pubIP.Current()
	}
	if !a.sup.Running() {
		return st
	}
	sessions, total, err := querySessions(a.sup.Port())
	if err != nil {
		// Console unreachable: report as not running with zeros.
		a.log.Logf("hbbr 控制台探活失败（端口 %d）：%v", a.sup.Port(), err)
		return st
	}
	st.HbbrRunning = true
	st.HbbrStartedAt = a.sup.StartedAt()
	st.Sessions = sessions
	st.TotalKbps = total
	return st
}

func (a *Agent) doHeartbeat() {
	ack := a.pendingAck
	a.pendingAck = ""
	plain, err := json.Marshal(heartbeatPlaintext{
		AppliedVersion: a.state.AppliedVersion,
		CommandAck:     ack,
		Status:         a.collectStatus(),
	})
	if err != nil {
		a.log.Logf("心跳组装失败：%v", err)
		return
	}
	enc, err := encryptBundle(a.linkKey, plain)
	if err != nil {
		a.log.Logf("心跳加密失败：%v", err)
		return
	}
	nonceHex, err := randomHex(16)
	if err != nil {
		a.log.Logf("心跳随机数生成失败：%v", err)
		return
	}
	ts := time.Now().Unix()
	sig := heartbeatSig(a.linkKey, a.state.NodeID, ts, nonceHex, enc.Nonce, enc.Data)
	resp, err := a.api.heartbeat(heartbeatRequest{
		NodeID: a.state.NodeID, TS: ts, Nonce: nonceHex, Sig: sig, Bundle: enc,
	})
	if err != nil {
		a.log.Logf("心跳失败（下个周期重试）：%v", err)
		// The ack was never delivered; keep it for the next attempt.
		if ack != "" {
			a.pendingAck = ack
		}
		return
	}
	if !resp.OK {
		if resp.Revoked {
			a.log.Logf("节点已被服务端删除/吊销，停止心跳，中继继续运行")
			a.setRevoked()
		} else {
			a.log.Logf("心跳被服务端拒绝（下个周期重试）：%s", resp.Error)
			// The server did not process this heartbeat, so the ack
			// was never delivered; keep it, or the command would be
			// re-sent and executed twice.
			if ack != "" {
				a.pendingAck = ack
			}
		}
		return
	}
	if resp.Bundle == nil {
		return
	}
	plain, err = decryptBundle(a.linkKey, *resp.Bundle)
	if err != nil {
		a.log.Logf("警告：心跳响应解密失败，不应用、不推进配置版本：%v", err)
		return
	}
	var upd heartbeatUpdate
	if err := json.Unmarshal(plain, &upd); err != nil {
		a.log.Logf("警告：心跳响应解析失败：%v", err)
		return
	}
	if upd.Config != nil && upd.ConfigVersion > a.state.AppliedVersion {
		a.applyConfig(*upd.Config, upd.ConfigVersion)
	}
	if upd.Command == "restart" {
		a.log.Logf("收到服务端 restart 命令，正在重启 hbbr")
		a.sup.RequestRestart(a.sup.Port(), a.sup.Limits())
		a.pendingAck = "restart"
	}
}

// applyConfig persists the pushed config, pushes the effective limits
// to the running hbbr over the loopback console (no restart needed),
// restarts hbbr only when the port changed, then records the new
// applied version.
func (a *Agent) applyConfig(cfg RelayConfig, version int) {
	a.log.Logf("收到新配置 version=%d：端口 %d，限速 %s/%s/%s",
		version, cfg.EffectivePort(),
		limitStr(cfg.LimitSpeed), limitStr(cfg.TotalBandwidth), limitStr(cfg.SingleBandwidth))
	env := cfg.ToEnv()
	if err := writeRelayEnv(a.dir, env); err != nil {
		a.log.Logf("relay.env 写入失败，配置未应用：%v", err)
		return
	}
	ls, tb, sb := cfg.EffectiveLimits()
	a.pushLimits(ls, tb, sb)
	if NeedsRestart(a.sup.Port(), cfg) {
		a.sup.RequestRestart(cfg.EffectivePort(), env.EnvMap())
	} else {
		// Limits already pushed live; keep the env overrides in
		// sync so the next (re)start uses the same values.
		a.sup.SetLimits(env.EnvMap())
	}
	a.state.AppliedVersion = version
	if err := saveAgentState(a.dir, a.state); err != nil {
		a.log.Logf("agent.json 保存失败：%v", err)
	}
}

func limitStr(v *float64) string {
	if v == nil {
		return "默认"
	}
	return fmt.Sprintf("%g", *v)
}

// pushLimits applies the effective limits to the running hbbr via its
// loopback console and verifies each value once afterwards. Any
// failure or mismatch is only logged: the limits are also persisted
// in relay.env, so the next hbbr start picks them up.
func (a *Agent) pushLimits(ls, tb, sb float64) {
	if !a.sup.Running() {
		a.log.Logf("hbbr 未运行，限速只写入 relay.env（下次启动生效）")
		return
	}
	port := a.sup.Port()
	for _, item := range []struct {
		cmd string
		val float64
	}{
		{"ls", ls}, {"tb", tb}, {"sb", sb},
	} {
		if err := setLimit(port, item.cmd, item.val); err != nil {
			a.log.Logf("限速下发失败 %s=%g：%v", item.cmd, item.val, err)
			continue
		}
		got, err := getLimit(port, item.cmd)
		if err != nil {
			a.log.Logf("限速核对失败 %s：%v", item.cmd, err)
		} else if math.Abs(got-item.val) > 0.001 {
			a.log.Logf("限速核对不一致 %s：期望 %g，实际 %g", item.cmd, item.val, got)
		}
	}
}
