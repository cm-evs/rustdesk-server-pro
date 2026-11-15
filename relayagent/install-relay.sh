#!/bin/sh
# Register this RustDesk relay node as a systemd service.
# Usage: sudo ./install-relay.sh
# The service runs relay-agent, which supervises hbbr: never start
# hbbr directly on a managed relay node (two processes would fight
# over the relay port). Register the node BEFORE starting the service:
#   ./relay-agent register --link <link from the console>
#   sudo systemctl start rustdesk-relay-agent
set -u
if [ "$(id -u)" -ne 0 ]; then
  if ! command -v sudo >/dev/null 2>&1; then
    echo "error: run this installer as root (sudo ./install-relay.sh)" >&2
    exit 1
  fi
  exec sudo "$0" "$@"
fi
DIR="$(cd "$(dirname "$0")" && pwd)"
if [ ! -x "$DIR/relay-agent" ] || [ ! -x "$DIR/hbbr" ]; then
  echo "error: relay-agent or hbbr not found in $DIR" >&2
  exit 1
fi
if ! command -v systemctl >/dev/null 2>&1; then
  echo "error: systemctl not found; this installer needs systemd" >&2
  exit 1
fi
if pgrep -x hbbr >/dev/null 2>&1; then
  echo "error: hbbr is already running. Stop the manually started instance first," >&2
  echo "then run this installer again (otherwise the service cannot bind the port)." >&2
  exit 1
fi
if [ -n "${SUDO_USER:-}" ]; then
  SERVICE_USER="$SUDO_USER"
else
  SERVICE_USER="$(stat -c %U "$DIR" 2>/dev/null || echo root)"
fi
UNIT=/etc/systemd/system/rustdesk-relay-agent.service
cat > "$UNIT" <<UNIT_EOF
[Unit]
Description=RustDesk Relay Agent (manages hbbr for rustdesk-server-pro)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$SERVICE_USER
WorkingDirectory=$DIR
ExecStart=$DIR/relay-agent run
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT_EOF
systemctl daemon-reload
systemctl enable rustdesk-relay-agent
echo "service installed (not started yet)."
if [ ! -f "$DIR/agent.json" ]; then
  echo "next step: register this node, then start the service:"
  echo "  cd $DIR"
  echo "  ./relay-agent register --link <link from the console Devices page>"
  echo "  sudo systemctl start rustdesk-relay-agent"
else
  if systemctl start rustdesk-relay-agent; then
    echo "service started (node was already registered)."
  else
    echo "error: service failed to start; see: journalctl -u rustdesk-relay-agent -n 100" >&2
    exit 1
  fi
fi
echo "status: systemctl status rustdesk-relay-agent"
echo "logs:   journalctl -u rustdesk-relay-agent -f"
echo "agent:  $DIR/logs/relay-agent.log"
