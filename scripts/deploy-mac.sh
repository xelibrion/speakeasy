#!/usr/bin/env bash
set -euo pipefail

# Ships the freshly built universal .app to a test Mac over SSH: copies the
# DMG, quits whatever instance is already running there, replaces
# /Applications, and relaunches. This is a local install onto a machine you
# already trust — the bundle is unsigned unless APPLE_SIGNING_IDENTITY was
# set for `make bundle-mac` (see the Makefile's notes on Accessibility
# grants needing a stable identity).

REMOTE="${DEPLOY_TARGET:?set DEPLOY_TARGET=user@host}"
APP_NAME="Speakeasy"
BUNDLE_DIR="target/universal-apple-darwin/release/bundle/dmg"

dmg=$(ls -t "$BUNDLE_DIR"/*.dmg 2>/dev/null | head -1)
if [ -z "$dmg" ]; then
	echo "no .dmg found in $BUNDLE_DIR — run 'make bundle-mac' first" >&2
	exit 1
fi

echo "==> $dmg -> $REMOTE:~"
scp "$dmg" "$REMOTE:~/"

ssh "$REMOTE" bash -s -- "$APP_NAME" "$(basename "$dmg")" <<'REMOTE_SCRIPT'
set -euo pipefail
app_name="$1"
dmg_name="$2"
dmg_path="$HOME/$dmg_name"

echo "==> quitting any running $app_name"
osascript -e "quit app \"$app_name\"" >/dev/null 2>&1 || true
pkill -x "speakeasy-desktop" >/dev/null 2>&1 || true
sleep 1

echo "==> mounting $dmg_path"
mount_point=$(hdiutil attach "$dmg_path" -nobrowse | awk -F'\t' '/\/Volumes\// {print $NF; exit}')

echo "==> installing to /Applications"
rm -rf "/Applications/$app_name.app"
cp -R "$mount_point/$app_name.app" /Applications/

echo "==> unmounting"
hdiutil detach "$mount_point" -quiet

rm -f "$dmg_path"

echo "==> relaunching"
open -a "$app_name"
REMOTE_SCRIPT

echo "==> done"
