#!/bin/sh
# The IPK a release ships: the open icon set only (never the local SF Symbols, whose license keeps them out of anything
# distributed), at the version in pkg/appinfo.json, with the license and the third-party notices beside the binary.
set -e
cd "$(dirname "$0")/.."
# no paths of this machine in the binary (panic locations name their source files): dependencies' sources map to
# /cargo, this checkout's to /crunchy
export RUSTFLAGS="--remap-path-prefix=$HOME/.cargo/registry/src=/cargo --remap-path-prefix=$HOME/.cargo/git/checkouts=/cargo-git --remap-path-prefix=$(pwd)=/crunchy"
CRUNCHY_ICONS=open ./tools/build.sh
# an SF Symbol's outline in the binary would mean the open set wasn't used
if grep -a -q "M0 156.80C0 168.23" build/app/crunchy; then
	echo "the release binary has SF Symbols in it" >&2
	exit 1
fi
cp LICENSE THIRD-PARTY-NOTICES.md build/app/
# the license texts that go with the code and assets the binary carries, and every crate's license
mkdir -p build/app/licenses
cp vendor/crunchyroll-rs/LICENSE-MIT build/app/licenses/crunchyroll-rs-MIT.txt
cp vendor/crunchyroll-rs/LICENSE-APACHE build/app/licenses/crunchyroll-rs-Apache-2.0.txt
cp assets/icons/LICENSE-framework7-icons.txt build/app/licenses/framework7-icons-MIT.txt
cp assets/fonts/Inter-OFL.txt build/app/licenses/inter-OFL.txt
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
# (the app's own crates out, and no local paths: "crunchyroll-rs v0.19.1 (/…/vendor/crunchyroll-rs)")
cargo tree -p crunchy-tv -e normal --target armv7-unknown-linux-gnueabi --prefix none -f "{p} — {l}" \
	| grep -v '^crunchy-' | sed 's| (/[^)]*)||' | sort -u > build/app/licenses/crates.txt
./tools/package.sh
