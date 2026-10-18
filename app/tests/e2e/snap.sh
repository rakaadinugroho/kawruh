#!/usr/bin/env bash
# Snapshots the webview of a running debug build to a PNG.
# Usage: snap.sh <port> <out.png>
port="$1"; out="$2"; tiff="${out%.png}.tiff"
curl -s -m 20 -X POST --data-binary "SNAPSHOT ${tiff}" "http://127.0.0.1:${port}/eval" >/dev/null
sips -s format png "$tiff" --out "$out" >/dev/null 2>&1 && rm -f "$tiff" && sips -Z 1600 "$out" >/dev/null && echo "$out"
