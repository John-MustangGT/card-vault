#!/usr/bin/env bash
# install-service.sh — build and register card-vault as a systemd service.
# Run as root (sudo ./install-service.sh) or a user with sudo access.

set -e

REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
SERVICE_NAME="card-vault"
SERVICE_FILE="$REPO_DIR/$SERVICE_NAME.service"
SYSTEMD_DIR="/etc/systemd/system"
RUN_USER="${SUDO_USER:-$USER}"

# ── 1. Build release binary ───────────────────────────────────────────────────
echo "==> Building release binary…"
sudo -u "$RUN_USER" bash -c "cd '$REPO_DIR' && cargo build --release"

# ── 2. Patch service file with real paths and user ───────────────────────────
INSTALL_SERVICE="$SYSTEMD_DIR/$SERVICE_NAME.service"
sed \
  -e "s|User=.*|User=$RUN_USER|" \
  -e "s|WorkingDirectory=.*|WorkingDirectory=$REPO_DIR|" \
  -e "s|EnvironmentFile=.*|EnvironmentFile=$REPO_DIR/.env|" \
  -e "s|ExecStart=.*|ExecStart=$REPO_DIR/target/release/$SERVICE_NAME|" \
  "$SERVICE_FILE" > "$INSTALL_SERVICE"

echo "==> Installed $INSTALL_SERVICE"

# ── 3. Reload systemd and enable ─────────────────────────────────────────────
systemctl daemon-reload
systemctl enable "$SERVICE_NAME"

echo ""
echo "Service installed.  Useful commands:"
echo "  sudo systemctl start   $SERVICE_NAME"
echo "  sudo systemctl stop    $SERVICE_NAME"
echo "  sudo systemctl restart $SERVICE_NAME"
echo "  sudo systemctl status  $SERVICE_NAME"
echo "  journalctl -u $SERVICE_NAME -f"
echo ""
echo "Edit $REPO_DIR/.env to change HOST, PORT, DATABASE_URL, DATA_DIR, etc."
echo "Then: sudo systemctl restart $SERVICE_NAME"
