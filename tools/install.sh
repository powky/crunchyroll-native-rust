#!/bin/sh
# Installs the newest IPK on the TV through its developer install service, over the launcher's ssh helper
# (~/dev/tv/launcher/tools/tv, which knows the TV's address and password), then launches it.
set -e
cd "$(dirname "$0")/.."
TV=${TV_SSH:-$HOME/dev/tv/launcher/tools/tv}
IPK=$(ls -t build/*.ipk | head -1)
VERSION=$(sed -n 's/.*"version": "\(.*\)".*/\1/p' pkg/appinfo.json)
ID=$(sed -n 's/.*"id": "\(.*\)".*/\1/p' pkg/appinfo.json)
$TV "cat > /tmp/crunchy.ipk" < "$IPK"
$TV "sleep 1 | luna-send -n 1 luna://com.webos.service.applicationmanager/closeByAppId '{\"id\":\"$ID\"}' >/dev/null 2>&1; sleep 1 | luna-send -n 1 luna://com.webos.appInstallService/dev/install '{\"id\":\"com.ares.defaultName\",\"ipkUrl\":\"/tmp/crunchy.ipk\",\"subscribe\":true}' >/dev/null"
for i in 1 2 3 4 5 6 7 8 9 10; do
  sleep 3
  if $TV "grep -q '\"version\": \"$VERSION\"' /media/developer/apps/usr/palm/applications/$ID/appinfo.json" 2>/dev/null; then
    echo "installed $VERSION"
    $TV "rm -f /tmp/crunchy.ipk; sleep 1 | luna-send -n 1 luna://com.webos.service.applicationmanager/launch '{\"id\":\"$ID\"}'"
    exit 0
  fi
done
echo "not installed yet" >&2
exit 1
