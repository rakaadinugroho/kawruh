#!/usr/bin/env bash
# Runs `kawruh-server admin …` on the homelab as the service user.
#
#   deploy/admin.sh list-users
#   deploy/admin.sh delete-user <username>     (stops the service while it runs)
#
# Host: KAWRUH_HOST=user@host (deploy/.env.local is read if present).
# Auth: SSH keys, or set KAWRUH_SSH_PASS to use sshpass.
set -euo pipefail
ENV_LOCAL="$(dirname "$0")/.env.local"
if [[ -f "$ENV_LOCAL" ]]; then source "$ENV_LOCAL"; fi
HOST="${KAWRUH_HOST:?set KAWRUH_HOST=user@host}"
SSH=(ssh -o StrictHostKeyChecking=accept-new -o ConnectTimeout=10)
if [[ -n "${KAWRUH_SSH_PASS:-}" ]]; then
  export SSHPASS="$KAWRUH_SSH_PASS"
  SSH=(sshpass -e "${SSH[@]}")
fi
CMD=$(printf '%q ' "$@")
STOP=false
[[ "${1:-}" == "delete-user" ]] && STOP=true
"${SSH[@]}" "$HOST" "STOP=$STOP bash -s -- $CMD" <<'REMOTE'
set -euo pipefail
$STOP && systemctl stop kawruh-server
trap '$STOP && systemctl start kawruh-server' EXIT
set -a; . /etc/kawruh/kawruh.env; set +a
runuser -u kawruh -- env KAWRUH_DATA="$KAWRUH_DATA" RUST_LOG=warn /opt/kawruh/kawruh-server admin "$@"
REMOTE
