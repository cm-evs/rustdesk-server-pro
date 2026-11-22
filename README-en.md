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
- **Template download**: hbbs has a built-in downloader with mirror fallback that fetches the latest official client as the generation template
- **ARM64 support**: aarch64 glibc and musl static packages; the musl build runs on OpenWrt soft-routers
- **Online one-click update**: check GitHub Releases from the console, read the full changelog, download and stage, auto-replace on restart
- **Other-platform download links**: official macOS / Linux / Android / iOS download links, plus a mirror-acceleration checkbox
- **One-click copy config**: generates a config string in the same format as the official Pro console's — paste it into the official client to import
- **Console Chinese/English switch**: Chinese by default; the choice is remembered
- **Require-login switch**: when on, clients that have not logged in cannot establish connections — TCP, UDP, and WebSocket are all blocked
- **Main-deployment relay key validation**: the main deployment's hbbr runs with `-k -`, so clients with a mismatched key are refused on relay
- **KX encryption on the 21116 control channel**: secure_tcp KX v0 encrypts login traffic; old clients can still connect in plaintext
- **Console URL state**: the current section is written to the address bar hash, so a refresh no longer returns to the overview

## Version changelog

- **Before v28**: the foundations — account system, web console, client generation, security hardening, and more
- **v28**: fixed the device database escaping its directory on Windows + service.bat menu
- **v29**: merged upstream relay UUID validation hardening
- **v30**: merged upstream hardening (message size limits, etc.); added ARM64 Linux packages (glibc / musl)
- **v31**: fixed "File name too long" for generated clients with long domains
- **v32**: online one-click update + API usage docs
- **v33**: other-platform official download links + one-click copy config
- **v34**: console Chinese/English switch + English deployment guide + download mirror checkbox
- **v35**: require-login switch
- **v36**: relay node management (registration + config push + status)
- **v37**: main-deployment relay key validation (hbbr with `-k -`)
- **v38**: KX encryption on the 21116 control channel (secure_tcp KX v0) + console URL state (hash routing; refresh no longer returns to the overview) + "Last relay used" column in the device list
- **v39**: admins create users directly in user management (works with registration closed; creating a user does not sign the admin out)
- **v40**: connection audit log (console "Audit Log" page: who/when/from where/to where/relayed or direct/duration)
- **v40.1 (preview)**: address book auto-adds bound devices ("登录设备" group; field parity with sctgdesk verified)
- **v41**: self-service password change (regular users change their own password in the console, old password verified; other devices are signed out after the change)
- **v42**: console UX fixes (Cancel button on the change-password panel, user list as the main view of user management) + template version check (skips re-downloading when the local template is already the latest version)
- **v43**: audit session reaping (sessions with a lost close record end by heartbeat, sessions on devices offline over 24 hours end as offline) + relay inference (public_host joins the relay IP set) + console tables rebuilt per the responsive-table guide (scroll container, sticky headers, stacked cards on narrow screens)
- **v44**: update-check button fix (the button is re-enabled after a no-update result) + token rename after creation (create returns the id, rename in the banner) + click truncated table cells to expand the full text (mouse, keyboard, and touch)
- **v45**: relay detail address falls back to the heartbeat IP + new listen port row + API Token page reworked as API key (one shared name input, "Add API key", no more "default" prefill) + --api-listen defaults to [::]:21114 (Refs #12, falls back to 0.0.0.0 when IPv6 cannot be bound) + Docker images (built by CI and published to GHCR; integer releases move latest)
- **v46**: console footer "About" block + sidebar navigation (left vertical collapsible sidebar, icon rail on narrow screens) + API key page name changed to remark (remark input in the banner right after the key) + relay agent self-reports its public IP (address display: registered address > reported public IP > heartbeat source IP; relays need a manual relay-agent swap) + "Check prereleases" checkbox in the update section (unchecked by default; preview builds carry the full version) + audit log date-range picker (from/to params, server-local time) + region-aware download order (mirrors first in China, direct GitHub first abroad)
- **v46.1 (preview)**: Docker runtime data lands in the volume (Fixes #13): RD_SKIP_ANCHOR=1 skips the working-directory anchor, set inside the s6 and classic images, and a key-secret-injected keypair is now read; the CI build on preview tags exports PRO_FULL_VERSION, so preview Docker images carry the full version; the smoke test is extended to a dual-image data-in-volume check; copy the data out of `/usr/bin` in the old container into the `/data` volume before upgrading, or it is lost on recreate; deployed relay nodes need no update

## Quick start

See [docs/DEPLOY-en.md](docs/DEPLOY-en.md) for deployment, upgrades, and per-version change notes.

## Relay nodes and distributed deployment

### Relay nodes

A relay node is a machine running hbbr, plus a resident agent (relay-agent). hbbr never connects to hbbs, so the agent handles registration, heartbeats, config updates, and supervising the local hbbr process.

- **Registration**: the console's Devices page generates a one-time registration link (valid 24 hours by default, shown only once); on the relay host, run `relay-agent register --link <link>`. Registration uses an HMAC challenge-response, and the server returns the node id, the server keypair, and the initial config encrypted with AES-256-GCM. Treat the link as a credential — delete it once used.
- **Key validation**: the agent starts hbbr as `hbbr -p <port> -k -`, forcing client key validation (without `-k`, hbbr validates no keys at all); if the key file is missing, it refuses to start hbbr — a keyless relay is never served.
- **Heartbeat and config**: the agent reports status every 20 seconds (encrypted, then signed); the console can push the listen port and three speed limits (a port change restarts hbbr; a speed-limit-only change applies live), and can restart hbbr remotely. Deleting a node revokes it: the agent stops heartbeating, hbbr keeps serving.
- **Console view**: admins see a relay sub-table on the Devices page: online/offline (judged by the last heartbeat within 90 seconds), address, versions, sessions, and throughput. Address display priority: registered address > agent-reported public IP > heartbeat source IP.
- **Registration is not rotation**: registration only establishes management — it does not add the node to the relay rotation list, which comes solely from the main server's `RELAY_SERVERS` startup parameter (below). Admins can add or remove a node on its relay detail page with the "Add to rotation / Remove from rotation" button — one click takes effect immediately and updates server.env; deleting a node removes it from the rotation list too.
- **No online updates**: relay nodes cannot update online — the agent only pushes config and restarts, never binaries; upgrading means manually replacing hbbr and relay-agent on the relay host (see "Relay deployment and updates").

### Distributed deployment

One main server (hbbs + API + console) plus relay nodes spread across sites or VPS hosts:

1. Deploy the main server as usual.
2. On each relay host, install and register relay-agent per the "Relay deployment and updates" section below. The relay port must be reachable from both peers (default 21117 TCP, WebSocket at port +2; forward the port if behind NAT); the agent reaches the main server's API (default 21114) over an outbound connection, so registration and heartbeats work from behind NAT.
3. Add the relay addresses, comma-separated, to `RELAY_SERVERS` in the main server's `server.env` (public IPs or publicly resolvable domains; a port may be included, default 21117), then restart the server (rdsvc reads server.env only at service start — restart the service, not just hbbs).
4. Clients fill in the relay field with the public address (no `https://`, port defaults to 21117 and may be omitted); only the relay field needs changing. Generated clients already carry the relay address.
5. Optional: for globally distributed relays, drop a MaxMind city database (`geoip-city.mmdb`) into the main server's directory (or point `RELAY_GEOIP_DB` at it) to enable nearest-relay selection (below); the startup log line `relay geo selection: enabled` confirms it.

Full steps and the registration/heartbeat protocol: the v36 section of [docs/DEPLOY-en.md](docs/DEPLOY-en.md) and [relayagent/README.md](relayagent/README.md).

### Relay deployment and updates

- **Deploy**: each platform has its own relay package (`rustdesk-server-pro-relay-*`) with hbbr, relay-agent, and install scripts. Linux: unzip, run `install-relay.sh` (installs the `rustdesk-relay-agent` systemd service), then `relay-agent register --link <link>`, then `systemctl start rustdesk-relay-agent`. Windows: unzip, run `install-service.bat` (registers the `RustDeskRelayAgent` service), then `relay-agent.exe register --link <link>`, then `service.bat start`. ARM soft-routers use the linux-aarch64-musl relay package (hbbr and agent are both statically linked). Only the agent starts hbbr — never start hbbr directly with systemd or rdsvc, or two processes will fight over the port.
- **Update**: stop the agent service; download the relay package for the new version from the GitHub release; unzip it to a temp directory; copy the new hbbr and relay-agent over the package directory (`agent.json`, `relay.env`, `id_ed25519`, and logs stay untouched — no re-registration needed); start the service. Relayed sessions on that node drop for a few seconds. Check the release notes: some versions need no relay update at all, and when only the agent changed, swapping relay-agent alone is enough.

### Relay allocation logic

hbbs only hands out relay addresses — it counts no sessions and does no pairing; pairing happens in hbbr.

- **Candidate list**: the list comes only from the `--relay-servers` startup parameter (`RELAY_SERVERS`), comma-separated, read at hbbs startup; registered relay nodes do not join it automatically, and a hand edit needs an hbbs restart; adding or removing through the console updates server.env and the live list together, taking effect immediately.
- **Health checks**: with 2 or more relays, hbbs TCP-probes each one every 3 seconds (3-second timeout); only reachable relays enter the allocation list, and if all probes fail the previous list is kept, not cleared. With a single relay there is no probing — it is always used.
- **When it is assigned**: when a controller asks hbbs to punch a hole to a peer, hbbs picks one relay by both peers' public IPs and writes it into the punch-hole message forwarded to the controlled peer; if direct connection fails, both peers go to that relay to pair.
- **Selection policy** (several relays): with a GeoIP database loaded, hbbs picks the relay minimizing the summed great-circle distance to both peers; relays within 1.3× + 250 km of the best form a near set that is rotated through. Without a database, when a peer cannot be located (private addresses, etc.), or when no relay can be located, it falls back to plain round-robin.
- **Client-side precedence** (verified against the official client 1.5.0 source): a client's own relay setting wins; if empty, the client uses the address hbbs assigned; if that is empty too, it uses the ID server's address with port +1 (21117 by default — the main server's own relay). Generated clients and clients with a relay filled in use their own relay; when neither side has one set, hbbs's single assigned address makes both peers pair at the same relay.
- **LAN cases**: when the controlled peer is on the LAN, hbbs hands out the server's LAN address; when one side is on the LAN and the other is not, relay is forced.

## Docker deployment

```bash
docker run -d --name rustdesk-pro \
  -p 21114:21114 -p 21115:21115 -p 21116:21116 -p 21116:21116/udp \
  -p 21117:21117 -p 21118:21118 -p 21119:21119 \
  -v rustdesk-pro-data:/data \
  -e RELAY=<your-server-address> \
  ghcr.io/wirh/rustdesk-server-pro:latest
```

The all-in-one image runs hbbs + hbbr together; the classic image (`ghcr.io/wirh/rustdesk-server-pro-classic`) runs one service per container — use it with the `docker-compose.yml` at the repo root. Docker Hub publishes the same images: `tesx/rustdesk-server-pro` and `tesx/rustdesk-server-pro-classic`. See [docs/DEPLOY-en.md](docs/DEPLOY-en.md) for the full deployment guide.

Runtime data lives in the `/data` volume (all-in-one image) or the mounted directory (classic image). Images v46 and earlier wrote the data into `/usr/bin` inside the container; before upgrading to v46.1, copy the old data out following the migration steps in [docs/DEPLOY-en.md](docs/DEPLOY-en.md), or it is lost when the container is recreated.

## Build

```bash
cargo build --release
```

This produces hbbs, hbbr, and rustdesk-utils. The Windows service host lives in `winservice/` and is built with Go.

Upstream base for this fork: rustdesk-server commit `873230e` (hbbs 1.1.17-dev).

## License

[AGPL-3.0](LICENSE). Based on the official open-source RustDesk server — thanks to the RustDesk team.
