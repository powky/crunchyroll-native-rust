#!/bin/sh
# Presses remote keys on the TV for testing: writes key events into the remote's evdev node, as if they came from
# it. Usage: tools/key.sh down down ok back (names below, or raw evdev codes); a number with "s" waits (0.5s).
set -e
TV=${TV_SSH:-$HOME/dev/tv/launcher/tools/tv}
DEV=${KEY_DEV:-/dev/input/event1}
# 16-byte input_event of the TV's 32-bit userland: timeval (zero), type, code, value
event() { printf '\\000\\000\\000\\000\\000\\000\\000\\000\\%03o\\000\\%03o\\%03o\\%03o\\000\\000\\000' "$1" $(($2 % 256)) $(($2 / 256)) "$3"; }
script=""
for key in "$@"; do
	case "$key" in
		*s) script="$script sleep ${key%s};"; continue ;;
		up) code=103 ;; down) code=108 ;; left) code=105 ;; right) code=106 ;;
		ok) code=28 ;; back) code=412 ;; exit) code=174 ;;
		*) code=$key ;;
	esac
	press="$(event 1 "$code" 1)$(event 0 0 0)"
	release="$(event 1 "$code" 0)$(event 0 0 0)"
	script="$script printf '$press' > $DEV; printf '$release' > $DEV; usleep 180000;"
done
$TV "$script" 2>&1 | grep -v 'Permission denied' || true
