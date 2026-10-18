#!/usr/bin/env bash
# Evaluates JS in a running debug build of Kawruh (KAWRUH_E2E_PORT).
# Usage: ev.sh <port> 'return document.title'   (or pipe the script on stdin)
port="$1"; shift
if [ $# -gt 0 ]; then body="$*"; else body="$(cat)"; fi
curl -s -m 70 -X POST --data-binary "$body" "http://127.0.0.1:${port}/eval"
echo
