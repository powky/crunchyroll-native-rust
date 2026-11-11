#!/bin/sh
# build/app -> build/com.ian.crunchy_<version>_arm.ipk
set -e
cd "$(dirname "$0")/.."
ARES=${ARES:-$HOME/dev/tv/launcher/tools/ares/node_modules/.bin}
"$ARES/ares-package" --no-minify build/app -o build/ | tail -1
ls build/*.ipk
