#!/usr/bin/env bash
# Snapshots the webview of a running debug build to a PNG.
# Usage: snap.sh <port> <out.png> [scale]   (scale re-renders at N× the window width, default 2)
port="$1"; out="$2"; scale="${3:-2}"; tiff="${out%.png}.tiff"
width=$(curl -s -m 10 -X POST --data-binary "return innerWidth * ${scale}" "http://127.0.0.1:${port}/eval" | sed -n 's/.*"value":"\([0-9.]*\)".*/\1/p')
curl -s -m 20 -X POST --data-binary "SNAPSHOT ${tiff} ${width}" "http://127.0.0.1:${port}/eval" >/dev/null
sips -s format png "$tiff" --out "$out" >/dev/null 2>&1 && rm -f "$tiff" && echo "$out"
