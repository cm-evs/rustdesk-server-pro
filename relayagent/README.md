# relay-agent — RustDesk 中继节点代理

常驻在中继机器上的代理：hbbr（中继服务器）从不连接 hbbs（ID 服务器），
所以由本代理负责向 hbbs 注册、心跳、上报状态、接收配置下发，并管理本地
hbbr 进程（启动/停止/重启）。

Go 实现，只用标准库 + `golang.org/x/sys`（Windows 服务与私钥文件 ACL 需要）。

## 文件布局（代理目录内）

| 文件 | 说明 |
| --- | --- |
| `relay-agent` / `relay-agent.exe` | 代理二进制 |
| `hbbr` / `hbbr.exe` | 中继二进制（与代理同目录） |
| `agent.json` | 注册凭证与同步状态（0600，见下） |
| `relay.env` | 代理管理的 hbbr 配置（`HBBR_PORT`、`LIMIT_SPEED`、`TOTAL_BANDWIDTH`、`SINGLE_BANDWIDTH`） |
| `id_ed25519` / `id_ed25519.pub` | 注册时服务端加密下发的密钥对（私钥 0600） |
| `logs/relay-agent.log` | 代理日志（追加，`2006-01-02 15:04:05` 时间戳） |
| `logs/hbbr.log` | hbbr 自己的日志（hbbr 内置 logger 写） |
| `logs/hbbr.out` | hbbr 子进程 stdout/stderr（另存一份，避免与 hbbr.log 重复） |
| `install-relay.sh` | Linux 安装脚本：注册 systemd 服务 `rustdesk-relay-agent` |
| `install-service.bat` / `service.bat` / `uninstall-service.bat` | Windows 服务安装/管理/卸载脚本（GBK + CRLF） |

## 用法

```text
relay-agent register --link <注册链接> [--name <名>] [--address <公网地址>]
relay-agent register --server <url> --token <rrt_...> --key <64hex> [--name <名>] [--address <公网地址>]
relay-agent run        前台运行：监督 hbbr 并向服务端心跳
relay-agent info       打印注册信息与 hbbr 状态
relay-agent version    打印代理版本
relay-agent install / uninstall / start / stop / restart / status  （仅 Windows，服务管理）
```

Unix 下无参数时等同于 `run`；Windows 下无参数时以服务方式运行
（服务名 `RustDeskRelayAgent`，显示名 `RustDesk Relay Agent`）。

注册流程（两步质询-响应）：

1. 代理用 `token_id`（`sha256(token)` 前 16 hex 字符）向服务端拉取一次性
   nonce（`POST /api/relay/challenge`）。
2. 代理算 `proof = HMAC-SHA256(link_key, "relay-register-v1\ntoken_id\nonce\nname\nagent_version\naddress")`
   并提交注册（`POST /api/relay/register`）。服务端返回 AES-256-GCM 加密的
   bundle，解密得节点 id、id_ed25519 密钥对与初始配置。
3. 代理落盘密钥对、写 `agent.json` 与 `relay.env`，打印节点 id 后退出。

nonce 一次性：register 因网络失败没收到响应时，代理不会用同一个 nonce
重试，而是重新拉 challenge 再注册（最多 3 次尝试）。token 已消耗时报错
提示删除控制台里的孤立节点后重新生成链接。

心跳（每 20 秒，首次立即发）：状态明文用 link_key 做 AES-256-GCM 加密，
请求带 `node_id`、`ts`、16 字节随机 nonce 与
`HMAC-SHA256("relay-heartbeat-v1\nnode_id\nts\nnonce\nbundle_nonce\nbundle_data")`
签名（encrypt-then-MAC，防重放）。服务端返回的 bundle 解密后是待下发的
配置与命令；解密失败只记日志，不应用、不推进 `applied_version`。

配置应用：端口变化 → 重启 hbbr；仅限速变化 → 走 hbbr 回环控制台实时生效
（`ls`/`tb`/`sb`，无参查询核对一次，不一致只记日志），不重启。限速为 null
时用 hbbr 内置默认值（32/1024/128 Mb/s）。`restart` 命令 → 重启 hbbr，
下次心跳带 `command_ack: "restart"`。

公网 IP 上报：服务端从心跳连接上拿不到中继的真实公网 IP（家用路由器会把
入站转发连接的源地址改写成网关内网 IP），所以代理在启动时通过外部查询服务
检测自身公网 IP（本地网卡只能看到私网 IP），随心跳上报；服务端按「注册地址
> 上报公网 IP > 心跳来源 IP」的优先级展示。查询服务依次尝试
`https://api.ipify.org`、`https://ifconfig.me/ip`、`https://icanhazip.com`，
单个 5 秒超时；检测失败保留上次结果，每小时重检一次。环境变量
`RELAY_PUBLIC_IP_URLS` 可覆盖查询服务（逗号分隔），设为空字符串则关闭检测。

节点被服务端删除/吊销（心跳返回 `revoked: true`）时，代理停止心跳但继续
监督 hbbr，不中断中继流量。

hbbr 启动方式：代理以 `hbbr -p <端口> -k -` 启动 hbbr。`-k -` 让 hbbr 从
下发的 `id_ed25519` 文件读取密钥对并强制校验客户端密钥——**不带 `-k` 时
hbbr 不校验任何密钥**（上游默认行为），所以 `-k -` 是必须的，不是可选的。
如果 `id_ed25519` 不存在，代理拒绝启动 hbbr（每 10 秒重试并记错误日志），
绝不放一个无密钥的中继出去；注意 `-k -` 在文件缺失时会自动生成新密钥对，
正是这个守卫要防的情况。

## 密钥保管

- **注册链接即凭证**：链接里同时包含一次性 token 与 32 字节对称密钥
  （link_key），拿到链接的人就能注册一个节点。注册成功后请删除聊天记录、
  终端历史里的链接，不要把它贴到工单、截图或公开场合。
- **link_key 存在 `agent.json`**（Linux 下 0600，仅所有者可读），它是节点
  的长期凭证：既用于解密服务端下发的配置，也用于签名每次心跳。不要把
  `agent.json` 发给别人；泄露后请在控制台删除该节点并重新注册。
  Windows 下 Go 的 Chmod 管不了 ACL，`agent.json` 只有默认 ACL（v1 未对它
  做 ACL 收紧；私钥文件做了），请把中继目录放在非共享位置。
- **id_ed25519 私钥**由服务端在注册时加密下发，落盘为 0600；Windows 下代理
  还会尽力把文件 ACL 收紧到仅当前用户 + SYSTEM（失败只记日志告警，不中断
  注册）。公钥 `id_ed25519.pub` 为 0644。
- 若目录里已有内容不同的密钥文件，注册时会先备份为 `id_ed25519.bak` /
  `id_ed25519.pub.bak` 再覆盖。
- token 本身与 link_key 绝不在任何请求/响应里出现：注册只传 `token_id` 与
  HMAC proof，心跳只传 HMAC 签名。
- 威胁模型：加密防被动窃听；有条件时仍建议给 API 配 TLS。

## 构建

```sh
export PATH=$HOME/workspace/tools/go/bin:$PATH
cd relayagent
go test ./...
go vet ./...
CGO_ENABLED=0 go build -o relay-agent-linux-amd64 .
CGO_ENABLED=0 GOOS=windows GOARCH=amd64 go build -o relay-agent-windows-amd64.exe .
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -o relay-agent-linux-arm64 .
```
