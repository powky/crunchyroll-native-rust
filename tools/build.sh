#!/bin/sh
# Release build for the TV (armv7, glibc 2.28) and the folder ares-package turns into an IPK: build/app
set -e
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo zigbuild -p crunchy-tv --release --target armv7-unknown-linux-gnueabi.2.28
rm -rf build/app && mkdir -p build/app/assets/fonts
cp target/armv7-unknown-linux-gnueabi/release/crunchy build/app/crunchy
cp pkg/appinfo.json pkg/icon.png pkg/largeIcon.png build/app/
cp assets/fonts/Inter-OFL.txt build/app/assets/fonts/
echo "build/app: $(du -sh build/app | cut -f1)"
