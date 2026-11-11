#!/bin/sh
# Dev loop once installed: rebuild, copy the binary over the installed one and relaunch.
set -e
cd "$(dirname "$0")/.."
TV=${TV_SSH:-$HOME/dev/tv/launcher/tools/tv}
./tools/build.sh >/dev/null
ID=$(sed -n 's/.*"id": "\(.*\)".*/\1/p' pkg/appinfo.json)
APP=/media/developer/apps/usr/palm/applications/$ID
$TV "sleep 1 | luna-send -n 1 luna://com.webos.service.applicationmanager/closeByAppId '{\"id\":\"$ID\"}' >/dev/null 2>&1; true"
$TV "cat > $APP/crunchy.new && chmod +x $APP/crunchy.new && mv $APP/crunchy.new $APP/crunchy" < build/app/crunchy
$TV "sleep 1 | luna-send -n 1 luna://com.webos.service.applicationmanager/launch '{\"id\":\"$ID\"}'"
echo pushed
