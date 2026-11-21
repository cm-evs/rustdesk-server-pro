package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

const httpTimeout = 10 * time.Second

type apiClient struct {
	base   string
	client *http.Client
}

func newAPIClient(server string) *apiClient {
	return &apiClient{
		base:   strings.TrimRight(server, "/"),
		client: &http.Client{Timeout: httpTimeout},
	}
}

// appError is a server-reported {"ok": false} failure, as opposed to a
// transport failure (no response received at all). Registration retry
// logic treats the two differently.
type appError struct{ msg string }

func (e *appError) Error() string { return e.msg }

// post sends a JSON request and decodes the JSON response. A transport
// failure or non-200 status is an error; an application-level
// {"ok": false} is decoded into respBody for the caller to inspect.
func (c *apiClient) post(path string, reqBody, respBody any) error {
	data, err := json.Marshal(reqBody)
	if err != nil {
		return err
	}
	resp, err := c.client.Post(c.base+path, "application/json", bytes.NewReader(data))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return err
	}
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("服务端返回 HTTP %d", resp.StatusCode)
	}
	if err := json.Unmarshal(body, respBody); err != nil {
		return fmt.Errorf("服务端响应解析失败: %w", err)
	}
	return nil
}

// --- challenge: POST /api/relay/challenge ---

type challengeResponse struct {
	OK    bool   `json:"ok"`
	Error string `json:"error"`
	Nonce string `json:"nonce"`
}

// challenge fetches a one-time server nonce for the registration
// challenge-response. An *appError means the token is unknown, used
// or expired (the server does not distinguish).
func (c *apiClient) challenge(tid string) (string, error) {
	var resp challengeResponse
	if err := c.post("/api/relay/challenge",
		map[string]string{"token_id": tid}, &resp); err != nil {
		return "", err
	}
	if !resp.OK {
		return "", &appError{msg: fmt.Sprintf("获取质询失败：%s", resp.Error)}
	}
	if resp.Nonce == "" {
		return "", &appError{msg: "获取质询失败：服务端未返回 nonce"}
	}
	return resp.Nonce, nil
}

// --- register: POST /api/relay/register ---

type registerRequest struct {
	TokenID      string `json:"token_id"`
	Nonce        string `json:"nonce"`
	Proof        string `json:"proof"`
	Name         string `json:"name"`
	AgentVersion string `json:"agent_version"`
	Address      string `json:"address"`
}

type registerResponse struct {
	OK     bool    `json:"ok"`
	Error  string  `json:"error"`
	Bundle *bundle `json:"bundle"`
}

// registrationBundle is the decrypted registration bundle: node id,
// the id_ed25519 keypair, and the initial config.
type registrationBundle struct {
	NodeID        int         `json:"node_id"`
	PrivateKey    string      `json:"private_key"`
	PublicKey     string      `json:"public_key"`
	Config        RelayConfig `json:"config"`
	ConfigVersion int         `json:"config_version"`
}

func (c *apiClient) register(req registerRequest) (*bundle, error) {
	var resp registerResponse
	if err := c.post("/api/relay/register", req, &resp); err != nil {
		return nil, err
	}
	if !resp.OK {
		return nil, &appError{msg: fmt.Sprintf("注册失败：%s", resp.Error)}
	}
	if resp.Bundle == nil {
		return nil, &appError{msg: "注册失败：服务端未返回配置包"}
	}
	return resp.Bundle, nil
}

// --- heartbeat: POST /api/relay/heartbeat ---

type heartbeatStatus struct {
	HbbrRunning   bool   `json:"hbbr_running"`
	HbbrStartedAt int64  `json:"hbbr_started_at"`
	HbbrVersion   string `json:"hbbr_version"`
	AgentVersion  string `json:"agent_version"`
	ListenPort    int    `json:"listen_port"`
	Sessions      int    `json:"sessions"`
	TotalKbps     int    `json:"total_kbps"`
	// PublicIP is the agent's own public IP, detected via an
	// external lookup service. Empty when detection is disabled
	// or has never succeeded; omitted from the JSON then, so the
	// server sees exactly what an old agent sends.
	PublicIP string `json:"public_ip,omitempty"`
}

// heartbeatPlaintext is the encrypted heartbeat payload.
type heartbeatPlaintext struct {
	AppliedVersion int             `json:"applied_version"`
	CommandAck     string          `json:"command_ack"`
	Status         heartbeatStatus `json:"status"`
}

type heartbeatRequest struct {
	NodeID int    `json:"node_id"`
	TS     int64  `json:"ts"`
	Nonce  string `json:"nonce"`
	Sig    string `json:"sig"`
	Bundle bundle `json:"bundle"`
}

type heartbeatResponse struct {
	OK      bool    `json:"ok"`
	Error   string  `json:"error"`
	Revoked bool    `json:"revoked"`
	Bundle  *bundle `json:"bundle"`
}

// heartbeatUpdate is the decrypted heartbeat response payload.
type heartbeatUpdate struct {
	Config        *RelayConfig `json:"config"`
	ConfigVersion int          `json:"config_version"`
	Command       string       `json:"command"`
}

func (c *apiClient) heartbeat(req heartbeatRequest) (*heartbeatResponse, error) {
	var resp heartbeatResponse
	if err := c.post("/api/relay/heartbeat", req, &resp); err != nil {
		return nil, err
	}
	return &resp, nil
}
