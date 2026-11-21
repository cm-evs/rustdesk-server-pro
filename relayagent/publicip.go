package main

import (
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"
)

// Public-IP self-report: the server cannot learn a relay's real
// public IP from the heartbeat connection — home routers rewrite the
// source of inbound forwarded connections to the gateway LAN IP
// (e.g. 10.0.0.1) — so the agent detects its own public IP through
// an external lookup service and reports it in the heartbeat status.
// Local interfaces only ever see private addresses, so the lookup
// must be external.
//
// The endpoints are plain-text IP services (one IP per response
// body). ipify first: a dedicated lookup service with a stable
// plain-text API; ifconfig.me and icanhazip.com are the fallbacks.
// Each attempt has a short timeout so a dead service never stalls
// startup for long; a failed detection keeps the last known value
// (empty when none was ever detected).
const publicIPTimeout = 5 * time.Second

// publicIPRecheckInterval: public IPs change rarely (DHCP lease /
// reconnect), so an hourly re-detect is plenty; it also bounds the
// load on the lookup services to one request per hour per node.
const publicIPRecheckInterval = time.Hour

var defaultPublicIPEndpoints = []string{
	"https://api.ipify.org",
	"https://ifconfig.me/ip",
	"https://icanhazip.com",
}

// publicIPEnvOverride: RELAY_PUBLIC_IP_URLS overrides the lookup
// endpoints (comma-separated). Set it to an empty string to disable
// public-IP detection entirely (air-gapped relays, tests).
const publicIPEnvOverride = "RELAY_PUBLIC_IP_URLS"

func publicIPEndpoints() []string {
	if v, ok := os.LookupEnv(publicIPEnvOverride); ok {
		var out []string
		for _, u := range strings.Split(v, ",") {
			if u = strings.TrimSpace(u); u != "" {
				out = append(out, u)
			}
		}
		return out
	}
	return defaultPublicIPEndpoints
}

// publicIPDetector caches the detected public IP and re-detects it
// periodically. Heartbeats read the cache, so a slow lookup never
// blocks the heartbeat loop.
type publicIPDetector struct {
	endpoints []string
	client    *http.Client
	interval  time.Duration
	mu        sync.Mutex
	ip        string
}

func newPublicIPDetector() *publicIPDetector {
	return &publicIPDetector{
		endpoints: publicIPEndpoints(),
		client:    &http.Client{Timeout: publicIPTimeout},
		interval:  publicIPRecheckInterval,
	}
}

// Current returns the last detected public IP ("" when none).
func (d *publicIPDetector) Current() string {
	d.mu.Lock()
	defer d.mu.Unlock()
	return d.ip
}

// refresh tries the endpoints in order and caches the first valid
// public IP. On total failure the cached value is kept. A no-op
// when detection is disabled (no endpoints configured).
func (d *publicIPDetector) refresh(log *logger) {
	if len(d.endpoints) == 0 {
		return
	}
	ip, err := detectPublicIP(d.client, d.endpoints)
	if err != nil {
		log.Logf("公网 IP 检测失败（保留上次结果）：%v", err)
		return
	}
	d.mu.Lock()
	changed := d.ip != "" && d.ip != ip
	first := d.ip == ""
	d.ip = ip
	d.mu.Unlock()
	if first {
		log.Logf("公网 IP：%s", ip)
	} else if changed {
		log.Logf("公网 IP 变化：%s", ip)
	}
}

// recheckLoop re-detects every interval until stopCh closes. Runs
// in its own goroutine; the first detection happens synchronously
// in Agent.Run so the first heartbeat already carries the IP.
func (d *publicIPDetector) recheckLoop(stopCh <-chan struct{}, log *logger) {
	if len(d.endpoints) == 0 {
		return
	}
	ticker := time.NewTicker(d.interval)
	defer ticker.Stop()
	for {
		select {
		case <-stopCh:
			return
		case <-ticker.C:
			d.refresh(log)
		}
	}
}

// detectPublicIP queries the endpoints in order and returns the
// first valid public IP. An endpoint whose body is not a single
// public IP (captive portal, error page) is skipped, not trusted.
func detectPublicIP(client *http.Client, endpoints []string) (string, error) {
	if len(endpoints) == 0 {
		return "", fmt.Errorf("未配置公网 IP 查询服务")
	}
	var lastErr error
	for _, u := range endpoints {
		ip, err := queryPublicIP(client, u)
		if err == nil {
			return ip, nil
		}
		lastErr = err
	}
	return "", lastErr
}

func queryPublicIP(client *http.Client, url string) (string, error) {
	resp, err := client.Get(url)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("%s 返回 HTTP %d", url, resp.StatusCode)
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, 128))
	if err != nil {
		return "", err
	}
	ip, ok := parsePublicIP(strings.TrimSpace(string(body)))
	if !ok {
		return "", fmt.Errorf("%s 返回的不是有效公网 IP", url)
	}
	return ip, nil
}

// parsePublicIP validates a lookup response: it must be one
// globally-routable unicast IP, not a private, loopback,
// link-local, multicast or unspecified address. A lookup service
// should never return one of those; if it does (captive portal,
// broken proxy), the value is unusable as the relay's public
// address. The returned string is the normalized form
// (IPv4-mapped IPv6 becomes plain IPv4).
func parsePublicIP(s string) (string, bool) {
	ip := net.ParseIP(s)
	if ip == nil || !ip.IsGlobalUnicast() || ip.IsPrivate() {
		return "", false
	}
	return ip.String(), true
}
