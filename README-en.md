# rustdesk-server-pro (self-hosted fork)

English | [中文](README.md)

A self-hosted remote desktop server based on the official open-source [rustdesk-server](https://github.com/rustdesk/rustdesk-server): an account system and web admin console on top of the official hbbs / hbbr; the official RustDesk client works with it without any modification.

> This project is an independent fork and is not affiliated with the official paid RustDesk Server Pro. License: AGPL-3.0, same as upstream.

📖 **[Chinese notes (with screenshots)](docs/中文说明.md)** | 📦 **[Deployment guide](docs/DEPLOY-en.md)** | 📦 **[部署文档（中文）](docs/DEPLOY.md)**

![Console device list](docs/images/console-devices.png)

## Extra features over the official open-source edition

- **Account system**: registration / login (the first registered account becomes admin); console login protected by an image captcha; the official client signs in with the same account, and devices are auto-bound to their owner account
- **Web console** (default port 21114): user management, device list, server settings, API token management; device and user lists are scoped per account — regular users only see their own
- **Generate client**: one-click generation of an official client with the server address and key pre-bound (server settings hidden in the client UI); each generation yields a tokenized download link, and deleting the generation record invalidates the link; any signed-in user can generate for themselves
- **Manual config card**: the console shows the ID server / relay server / API server / key directly, so existing official clients can join by filling in the four values
- **Token API**: API tokens with the `rdt_` prefix; the server only stores their SHA-256 hash
- **Security hardening**: brute-force lockout on login (per-account + per-IP), an open-registration switch, separate listeners for the management plane and client endpoints (`--console-listen`), optional HTTPS (`--tls-cert` / `--tls-key`)
- **Heartbeat strategy push**: pushes "hide server settings" to clients, optionally "allow remote config modification"
- **Geo-nearest multi-relay selection**: optional MaxMind city-level GeoIP database (`geoip-city.mmdb`); picks the nearest relay by the summed geographic distance to both peers, falling back to the official round-robin when the database is missing or coordinates cannot be resolved
- **Relay node management**: a resident agent (relay-agent) runs on each relay host; the console generates a one-time registration link to enroll it; the server pushes the keypair and config (port, speed limits, restart) over encrypted channels, and the device list shows relay node status (online/offline, sessions, throughput) — click a node name for details
- **Windows service support**: `winservice/` provides the rdsvc.exe service host and management scripts to register hbbs / hbbr as auto-start Windows services
- **Runtime files stay put**: everything the server produces at runtime (databases, logs, keys, etc.) lands in the server program's own directory

## Quick start

See [docs/DEPLOY-en.md](docs/DEPLOY-en.md) for deployment, upgrades, and per-version change notes.

## Build

```bash
cargo build --release
```

This produces hbbs, hbbr, and rustdesk-utils. The Windows service host lives in `winservice/` and is built with Go.

Upstream base for this fork: rustdesk-server commit `873230e` (hbbs 1.1.17-dev).

## License

[AGPL-3.0](LICENSE). Based on the official open-source RustDesk server — thanks to the RustDesk team.
