#!/bin/sh
# Usage: shoot.sh <name> <query> [height]  -> docs-site/images/<name>.png (run from repo root)
EDGE="/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"
OUT="$(cygpath -w "$PWD/manual/images/$1.png")"
"$EDGE" --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=2 \
  --window-size=${4:-720},${3:-900} --virtual-time-budget=6000 --user-data-dir="$TEMP/vw-shots" \
  --screenshot="$OUT" "http://localhost:1430/scripts/manual-shots/index.html?$2" 2>/dev/null
ls -la "manual/images/$1.png"
