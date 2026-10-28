#!/usr/bin/env bash
set -euo pipefail

# Ships the freshly built universal .app to a test Mac over SSH: copies the
# DMG, quits whatever instance is already running there, replaces
# /Applications, and relaunches. This is a local install onto a machine you
# already trust. `make deploy-mac` signs the bundle first (Accessibility grants
# need a stable identity), and this script installs a LaunchAgent so Speakeasy
# starts at every login.

REMOTE="${DEPLOY_TARGET:?set DEPLOY_TARGET=user@host}"
APP_NAME="Speakeasy"
BUNDLE_DIR="target/universal-apple-darwin/release/bundle/dmg"

dmg=$(ls -t "$BUNDLE_DIR"/*.dmg 2>/dev/null | head -1)
if [ -z "$dmg" ]; then
	echo "no .dmg found in $BUNDLE_DIR — run 'make bundle-mac' first" >&2
	exit 1
fi

# A .local name resolves to several addresses, including an unscoped IPv6
# link-local one. Without a connect timeout ssh gives up on that first address
# ("connect ... Undefined error: 0"); with one it falls through to a working one.
ssh_opts=(-o ConnectTimeout=10)

echo "==> $dmg -> $REMOTE:~"
scp "${ssh_opts[@]}" "$dmg" "$REMOTE:~/"

ssh "${ssh_opts[@]}" "$REMOTE" bash -s -- "$APP_NAME" "$(basename "$dmg")" <<'REMOTE_SCRIPT'
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

echo "==> installing launch-at-login agent"
label="com.speakeasy.app.autostart"
plist="$HOME/Library/LaunchAgents/$label.plist"
mkdir -p "$HOME/Library/LaunchAgents"
cat >"$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>$label</string>
	<key>ProgramArguments</key>
	<array>
		<string>/usr/bin/open</string>
		<string>-a</string>
		<string>/Applications/$app_name.app</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
</dict>
</plist>
PLIST
# Reload so an edited plist takes effect; RunAtLoad launches the app now, which
# also covers the relaunch. `open` goes through LaunchServices, so TCC
# attributes Accessibility to the app, not to launchd.
launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true
launchctl bootstrap "gui/$(id -u)" "$plist"
REMOTE_SCRIPT

echo "==> done"
