#!/bin/sh
# Saves what the TV shows as a PNG: tools/shot.sh out.png (also writes out-s.png at half size).
set -e
TV=${TV_SSH:-$HOME/dev/tv/launcher/tools/tv}
OUT=${1:?usage: tools/shot.sh out.png}
$TV 'rm -f /tmp/cap.png; sleep 1 | luna-send -n 1 luna://com.webos.service.capture/executeOneShot "{\"path\":\"/tmp/cap.png\",\"method\":\"SCREEN\",\"width\":1920,\"height\":1080,\"format\":\"PNG\"}" >/dev/null; sleep 3' 2>&1 | grep -v 'Permission denied' || true
$TV 'cat /tmp/cap.png' > "$OUT" 2>/dev/null
sips -Z 960 "$OUT" --out "${OUT%.png}-s.png" >/dev/null
