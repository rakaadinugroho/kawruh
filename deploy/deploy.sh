#!/usr/bin/env bash
# Cross-compiles kawruh-server for the homelab (aarch64, static musl) and
# installs it as the `kawruh-server` systemd service.
#
#   deploy/deploy.sh [user@host]                  build, upload, install, restart
#   deploy/deploy.sh --release vX.Y.Z [user@host] install a GitHub release build instead
#   deploy/deploy.sh --rollback [user@host]       restore the previous binary
#
# Host: the user@host argument, else KAWRUH_HOST (deploy/.env.local is read if present).
# Auth: SSH keys, or set KAWRUH_SSH_PASS to use sshpass.
# Data in /var/lib/kawruh and /etc/kawruh/kawruh.env are never overwritten.
set -euo pipefail
ENV_LOCAL="$(dirname "$0")/.env.local"
if [[ -f "$ENV_LOCAL" ]]; then source "$ENV_LOCAL"; fi

ROLLBACK=false
RELEASE=""
case "${1:-}" in
  --rollback) ROLLBACK=true; shift ;;
  --release) RELEASE="${2:?usage: deploy.sh --release vX.Y.Z [user@host]}"; shift 2 ;;
esac
HOST="${1:-${KAWRUH_HOST:?set KAWRUH_HOST=user@host or pass user@host}}"
PUBLIC_URL="${KAWRUH_PUBLIC_URL:-http://${HOST#*@}:7420}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET=aarch64-unknown-linux-musl
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/server}"
BIN="$CARGO_TARGET_DIR/$TARGET/release/kawruh-server"

SSH=(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10)
SCP=(scp -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10)
if [[ -n "${KAWRUH_SSH_PASS:-}" ]]; then
  export SSHPASS="$KAWRUH_SSH_PASS"
  SSH=(sshpass -e "${SSH[@]}")
  SCP=(sshpass -e "${SCP[@]}")
fi

health() {
  for _ in 1 2 3 4 5; do
    if curl -fsS "$PUBLIC_URL/api/health"; then echo; return 0; fi
    sleep 1
  done
  echo "health check failed: $PUBLIC_URL/api/health" >&2
  return 1
}

if $ROLLBACK; then
  echo "==> rolling back on $HOST"
  "${SSH[@]}" "$HOST" 'set -e
    test -f /opt/kawruh/kawruh-server.prev || { echo "no previous binary" >&2; exit 1; }
    cp -f /opt/kawruh/kawruh-server.prev /opt/kawruh/kawruh-server
    systemctl restart kawruh-server && systemctl is-active kawruh-server'
  health
  exit 0
fi

if [[ -n "$RELEASE" ]]; then
  echo "==> downloading $RELEASE ($TARGET) from GitHub"
  ASSET="kawruh-server-$RELEASE-$TARGET.tar.gz"
  DL="$(mktemp -d)"
  trap 'rm -rf "$DL"' EXIT
  (cd "$ROOT" && gh release download "$RELEASE" --dir "$DL" --pattern "$ASSET" --pattern SHA256SUMS.txt)
  (cd "$DL" && grep " $ASSET\$" SHA256SUMS.txt | shasum -a 256 -c -)
  tar -xzf "$DL/$ASSET" -C "$DL"
  BIN="$DL/kawruh-server"
else
  echo "==> building $TARGET"
  (cd "$ROOT" && cargo zigbuild --release --target "$TARGET" -p kawruh-server)
fi

echo "==> checking free disk on $HOST"
FREE_KB=$("${SSH[@]}" "$HOST" "df -Pk / | awk 'NR==2{print \$4}'")
BIN_KB=$(du -k "$BIN" | cut -f1)
NEED_KB=$(( BIN_KB * 3 + 60 * 1024 ))
if (( FREE_KB < NEED_KB )); then
  echo "not enough disk on $HOST: ${FREE_KB} KB free, need ${NEED_KB} KB" >&2
  exit 1
fi
echo "    ${FREE_KB} KB free"

echo "==> uploading"
"${SCP[@]}" "$BIN" "$HOST:/tmp/kawruh-server.new"
"${SCP[@]}" "$ROOT/deploy/kawruh-server.service" "$HOST:/tmp/kawruh-server.service"

echo "==> installing"
"${SSH[@]}" "$HOST" "PUBLIC_URL='$PUBLIC_URL' bash -s" <<'REMOTE'
set -euo pipefail
id kawruh >/dev/null 2>&1 || useradd --system --home-dir /var/lib/kawruh --shell /usr/sbin/nologin kawruh
install -d -o root -g root -m 755 /opt/kawruh /etc/kawruh
install -d -o kawruh -g kawruh -m 750 /var/lib/kawruh
if [[ -f /opt/kawruh/kawruh-server ]]; then
  cp -f /opt/kawruh/kawruh-server /opt/kawruh/kawruh-server.prev
fi
install -o root -g root -m 755 /tmp/kawruh-server.new /opt/kawruh/kawruh-server
rm -f /tmp/kawruh-server.new
if [[ ! -f /etc/kawruh/kawruh.env ]]; then
  cat > /etc/kawruh/kawruh.env <<ENV
KAWRUH_BIND=0.0.0.0:7420
KAWRUH_DATA=/var/lib/kawruh
KAWRUH_PUBLIC_URL=$PUBLIC_URL
KAWRUH_ALLOW_SIGNUP=true
RUST_LOG=info
ENV
  chmod 644 /etc/kawruh/kawruh.env
fi
install -o root -g root -m 644 /tmp/kawruh-server.service /etc/systemd/system/kawruh-server.service
rm -f /tmp/kawruh-server.service
if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
  ufw allow 7420/tcp >/dev/null
fi
systemctl daemon-reload
systemctl enable kawruh-server >/dev/null 2>&1
systemctl restart kawruh-server
sleep 1
systemctl is-active kawruh-server
REMOTE

echo "==> health"
health
