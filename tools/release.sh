#!/bin/sh
# The IPK a release ships: the open icon set only (never the local SF Symbols, whose license keeps them out of anything
# distributed), at the version in pkg/appinfo.json, with the third-party notices beside the binary.
set -e
cd "$(dirname "$0")/.."
CRUNCHY_ICONS=open ./tools/build.sh
# an SF Symbol's outline in the binary would mean the open set wasn't used
if grep -a -q "M0 156.80C0 168.23" build/app/crunchy; then
	echo "the release binary has SF Symbols in it" >&2
	exit 1
fi
cp THIRD-PARTY-NOTICES.md build/app/
./tools/package.sh
