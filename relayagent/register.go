package main

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"net/url"
	"os"
	"strings"
)

// parseRegisterLink extracts the server base URL, the one-time token
// and the 32-byte link key from a registration link such as
// http://host:21114/relay/register?token=rrt_...&key=<64hex>.
func parseRegisterLink(link string) (server, token string, linkKey []byte, err error) {
	u, err := url.Parse(link)
	if err != nil {
		return "", "", nil, fmt.Errorf("注册链接无效：%v", err)
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return "", "", nil, fmt.Errorf("注册链接无效：须以 http:// 或 https:// 开头")
	}
	if u.Host == "" {
		return "", "", nil, fmt.Errorf("注册链接无效：缺少主机地址")
	}
	token = u.Query().Get("token")
	if token == "" {
		return "", "", nil, fmt.Errorf("注册链接无效：缺少 token 参数")
	}
	if !strings.HasPrefix(token, "rrt_") {
		return "", "", nil, fmt.Errorf("注册链接无效：token 应以 rrt_ 开头")
	}
	keyHex := u.Query().Get("key")
	if keyHex == "" {
		return "", "", nil, fmt.Errorf("注册链接无效：缺少 key 参数")
	}
	linkKey, err = parseLinkKey(keyHex)
	if err != nil {
		return "", "", nil, fmt.Errorf("注册链接无效：%v", err)
	}
	return u.Scheme + "://" + u.Host, token, linkKey, nil
}

// registerAttempts bounds the challenge/register retry loop. Only
// transport failures are retried, each with a fresh challenge nonce.
const registerAttempts = 3

const consumedLinkHint = "注册链接已使用，如未拿到密钥请删除控制台里的孤立节点后重新生成链接"

func doRegister(dir string, args []string) int {
	log := newLogger(dir, true)
	fs := flag.NewFlagSet("register", flag.ContinueOnError)
	link := fs.String("link", "", "注册链接")
	server := fs.String("server", "", "服务端地址，如 http://host:21114")
	token := fs.String("token", "", "一次性注册 token（rrt_...）")
	keyHex := fs.String("key", "", "注册链接里的对称密钥（64 hex）")
	name := fs.String("name", "", "节点名（默认取主机名）")
	address := fs.String("address", "", "公网地址（可空）")
	fs.Usage = func() {
		fmt.Println("用法：")
		fmt.Println("  relay-agent register --link <注册链接> [--name <名>] [--address <公网地址>]")
		fmt.Println("  relay-agent register --server <url> --token <rrt_...> --key <64hex> [--name <名>] [--address <公网地址>]")
	}
	if err := fs.Parse(args); err != nil {
		return 1
	}
	// Refuse to overwrite an existing registration.
	if st, err := loadAgentState(dir); err == nil && st.LinkKey != "" {
		fmt.Printf("已注册过节点 %d（%s），如需重新注册请先删除 agent.json。\n", st.NodeID, st.Name)
		return 1
	} else if err != nil && err != errNotRegistered {
		fmt.Printf("agent.json 无法读取（%v），请删除后重试。\n", err)
		return 1
	}
	srv, tok, linkKey := *server, *token, []byte(nil)
	if *link != "" {
		s, t, k, err := parseRegisterLink(*link)
		if err != nil {
			fmt.Println("错误：", err)
			return 1
		}
		srv, tok, linkKey = s, t, k
	} else {
		if *keyHex == "" {
			fmt.Println("错误：须提供 --link，或同时提供 --server、--token 与 --key。")
			fs.Usage()
			return 1
		}
		k, err := parseLinkKey(*keyHex)
		if err != nil {
			fmt.Println("错误：--key 无效：", err)
			return 1
		}
		linkKey = k
	}
	if srv == "" || tok == "" {
		fmt.Println("错误：须提供 --link，或同时提供 --server、--token 与 --key。")
		fs.Usage()
		return 1
	}
	nodeName := *name
	if nodeName == "" {
		if host, err := os.Hostname(); err == nil && host != "" {
			nodeName = host
		} else {
			nodeName = "relay"
		}
	}

	api := newAPIClient(srv)
	tid := tokenID(tok)
	var enc *bundle
	ambiguous := false // a register attempt may have been processed
	for attempt := 1; attempt <= registerAttempts; attempt++ {
		nonce, err := api.challenge(tid)
		if err != nil {
			var appErr *appError
			if errors.As(err, &appErr) {
				// Token unknown / used / expired: retrying cannot help.
				fmt.Println("错误：", err)
				if ambiguous {
					fmt.Println("提示：", consumedLinkHint)
				}
				return 1
			}
			log.Logf("获取质询失败（网络错误，第 %d/%d 次尝试）：%v", attempt, registerAttempts, err)
			continue
		}
		proof := registerProof(linkKey, tid, nonce, nodeName, agentVersion, *address)
		enc, err = api.register(registerRequest{
			TokenID: tid, Nonce: nonce, Proof: proof,
			Name: nodeName, AgentVersion: agentVersion, Address: *address,
		})
		if err != nil {
			var appErr *appError
			if errors.As(err, &appErr) {
				fmt.Println("错误：", err)
				if ambiguous {
					fmt.Println("提示：", consumedLinkHint)
				}
				return 1
			}
			// No response received: the nonce is spent and the
			// token may already be consumed. Never reuse the
			// nonce — fetch a fresh challenge and try again.
			ambiguous = true
			log.Logf("注册请求未收到响应（网络错误，第 %d/%d 次尝试），将重新拉取质询重试：%v", attempt, registerAttempts, err)
			continue
		}
		break
	}
	if enc == nil {
		fmt.Println("错误：注册请求多次失败，未收到服务端响应。")
		fmt.Println("提示：", consumedLinkHint)
		return 1
	}
	plain, err := decryptBundle(linkKey, *enc)
	if err != nil {
		log.Logf("注册失败：配置包解密失败，可能有人篡改或密钥不对：%v", err)
		fmt.Println("错误：配置包解密失败，可能有人篡改或密钥不对。")
		return 1
	}
	var reg registrationBundle
	if err := json.Unmarshal(plain, &reg); err != nil {
		log.Logf("注册失败：配置包内容解析失败：%v", err)
		fmt.Println("错误：配置包内容解析失败。")
		return 1
	}
	if err := writeKeyFiles(dir, reg.PrivateKey, reg.PublicKey, log); err != nil {
		fmt.Println("错误：", err)
		return 1
	}
	st := &AgentState{
		Server:         strings.TrimRight(srv, "/"),
		NodeID:         reg.NodeID,
		LinkKey:        hex.EncodeToString(linkKey),
		Name:           nodeName,
		Address:        *address,
		AppliedVersion: reg.ConfigVersion,
	}
	if err := saveAgentState(dir, st); err != nil {
		fmt.Println("错误：agent.json 保存失败：", err)
		return 1
	}
	if err := writeRelayEnv(dir, reg.Config.ToEnv()); err != nil {
		fmt.Println("错误：relay.env 保存失败：", err)
		return 1
	}
	fmt.Printf("注册成功：节点 id=%d，名称=%s，端口=%d\n", reg.NodeID, nodeName, reg.Config.EffectivePort())
	return 0
}
