#!/usr/bin/env bash
# install.sh — deploy quantum-jobs binary and systemd service
# Usage: sudo ./deploy/install.sh
set -euo pipefail

BINARY="${1:-target/release/quantum-jobs}"
SERVICE="deploy/quantum-jobs.service"

if [[ ! -f "$BINARY" ]]; then
    echo "Binary not found: $BINARY"
    echo "Run: cargo build --release"
    exit 1
fi

echo "Installing quantum-jobs..."
systemctl stop quantum-jobs 2>/dev/null || true
cp "$BINARY" /usr/local/bin/quantum-jobs
chmod 755 /usr/local/bin/quantum-jobs
cp "$SERVICE" /etc/systemd/system/quantum-jobs.service
systemctl daemon-reload
systemctl enable quantum-jobs
systemctl start quantum-jobs
systemctl is-active quantum-jobs
echo "quantum-jobs deployed and running on port 8766"
