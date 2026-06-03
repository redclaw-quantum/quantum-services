#!/usr/bin/env bash
# install.sh — deploy quantum-api binary and systemd service
# Usage: sudo ./deploy/install.sh
set -euo pipefail

BINARY="${1:-target/release/quantum-api}"
SERVICE="deploy/quantum-api.service"

if [[ ! -f "$BINARY" ]]; then
    echo "Binary not found: $BINARY"
    echo "Run: cargo build --release"
    exit 1
fi

echo "Installing quantum-api..."
systemctl stop quantum-api 2>/dev/null || true
cp "$BINARY" /usr/local/bin/quantum-api
chmod 755 /usr/local/bin/quantum-api
cp "$SERVICE" /etc/systemd/system/quantum-api.service
systemctl daemon-reload
systemctl enable quantum-api
systemctl start quantum-api
systemctl is-active quantum-api
echo "quantum-api deployed and running on port 8765"
