#!/usr/bin/env bash
# Runs `kawruh-server admin …` on the homelab as the service user.
#
#   deploy/admin.sh list-users
#   deploy/admin.sh invite                     prints a single-use sign-up code
#   deploy/admin.sh delete-user <username>     (stops the service while it runs)
#   deploy/admin.sh gc [--dry-run]             (stops the service while it runs)
#
# `gc` frees disk: images no page refers to any more and the documents of
# pages purged from the trash. It keeps unreferenced images for a week first,
# so a page can still be restored. Always try `--dry-run` first.
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
case "${1:-}" in
  delete-user | gc) STOP=true ;;
esac
"${SSH[@]}" "$HOST" "STOP=$STOP bash -s -- $CMD" <<'REMOTE'
set -euo pipefail
$STOP && systemctl stop kawruh-server
trap '$STOP && systemctl start kawruh-server' EXIT
set -a; . /etc/kawruh/kawruh.env; set +a
runuser -u kawruh -- env KAWRUH_DATA="$KAWRUH_DATA" RUST_LOG=warn /opt/kawruh/kawruh-server admin "$@"
REMOTE
