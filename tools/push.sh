#!/bin/sh
# Dev loop once installed: rebuild, copy the binary over the installed one and relaunch.
set -e
cd "$(dirname "$0")/.."
TV=${TV_SSH:-$HOME/dev/tv/launcher/tools/tv}
./tools/build.sh >/dev/null
APP=/media/developer/apps/usr/palm/applications/com.ian.crunchy
$TV 'sleep 1 | luna-send -n 1 luna://com.webos.service.applicationmanager/closeByAppId "{\"id\":\"com.ian.crunchy\"}" >/dev/null 2>&1; true'
$TV "cat > $APP/crunchy.new && chmod +x $APP/crunchy.new && mv $APP/crunchy.new $APP/crunchy" < build/app/crunchy
$TV 'sleep 1 | luna-send -n 1 luna://com.webos.service.applicationmanager/launch "{\"id\":\"com.ian.crunchy\"}"'
echo pushed
