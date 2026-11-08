#!/bin/sh
# Runs scripts/dashboard.sh under launchd (macOS): the fallback when GitHub Pages is not there
# (docs/PLAN-phase2.md, P4; docs/dashboard.md, "Without Pages").
#
#   scripts/dashboard-agent.sh install <name>
#   scripts/dashboard-agent.sh uninstall <name>
#   scripts/dashboard-agent.sh status <name>
#
# `install` writes the LaunchAgent com.varswap.dashboard.<name> and loads it. It runs this
# checkout's scripts/dashboard.sh every hour and at login, into $VARSWAP_KEEPER_HOME/<name>/
# (default ~/.local/share/varswap-keeper): serve <home>/<name>/out/current. A run that finds
# another one holding the directory exits 1, and the next hour tries again. A run killed with
# SIGKILL leaves the lock, and every later run fails until it is removed (docs/dashboard.md).
#
# This checkout and its target/release/varswap-admin (VARSWAP_ADMIN_BIN overrides it) are used
# in place: move or rebuild them and install again. node is the one in PATH at install time.
# SOLANA_URL, if set, is written into the plist; it must hold no key, since a plist is no place
# for one, and it defaults to devnet's public RPC. Output goes to <home>/<name>/dashboard.log.

set -eu

usage() {
	sed -n '5,7p' "$0" | sed 's/^# *//' >&2
	exit 2
}

[ $# -eq 2 ] || usage
cmd=$1
name=$2
case $name in
*[!A-Za-z0-9_-]* | '') echo "name must be letters, digits, - or _" >&2 && exit 2 ;;
esac

home=${VARSWAP_KEEPER_HOME:-$HOME/.local/share/varswap-keeper}
dir=$home/$name
label=com.varswap.dashboard.$name
plist=$HOME/Library/LaunchAgents/$label.plist
uid=$(id -u)
if launchctl print "gui/$uid" >/dev/null 2>&1; then
	domain=gui/$uid
else
	domain=user/$uid
fi

loaded() {
	for d in "gui/$uid" "user/$uid"; do
		if launchctl print "$d/$label" >/dev/null 2>&1; then
			echo "$d"
		fi
	done
}

unload() {
	for d in $(loaded); do
		if ! launchctl bootout "$d/$label" 2>/dev/null; then
			if launchctl print "$d/$label" >/dev/null 2>&1; then
				echo "could not unload $label from $d" >&2
				exit 1
			fi
		fi
	done
}

xml() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

case $cmd in
install)
	root=$(cd "$(dirname "$0")/.." && pwd)
	admin=${VARSWAP_ADMIN_BIN:-$root/target/release/varswap-admin}
	[ -x "$admin" ] || { echo "$admin missing: cargo build --release -p varswap-cli" >&2 && exit 1; }
	node=$(command -v node) || { echo "node is not in PATH" >&2 && exit 1; }
	rpc=${SOLANA_URL:-https://api.devnet.solana.com}
	mkdir -p "$dir" "$HOME/Library/LaunchAgents"
	cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>$label</string>
	<key>ProgramArguments</key>
	<array>
		<string>/bin/sh</string>
		<string>$(xml "$root/scripts/dashboard.sh")</string>
		<string>$(xml "$dir/out")</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>PATH</key><string>$(xml "$(dirname "$node"):/usr/bin:/bin")</string>
		<key>SOLANA_URL</key><string>$(xml "$rpc")</string>
		<key>VARSWAP_ADMIN_BIN</key><string>$(xml "$admin")</string>
	</dict>
	<key>LimitLoadToSessionType</key>
	<array>
		<string>Aqua</string>
		<string>Background</string>
	</array>
	<key>RunAtLoad</key><true/>
	<key>StartInterval</key><integer>3600</integer>
	<key>StandardOutPath</key><string>$(xml "$dir/dashboard.log")</string>
	<key>StandardErrorPath</key><string>$(xml "$dir/dashboard.log")</string>
	<key>WorkingDirectory</key><string>$(xml "$dir")</string>
</dict>
</plist>
EOF
	plutil -lint "$plist" >/dev/null
	unload
	launchctl bootstrap "$domain" "$plist"
	echo "installed $label: $plist"
	echo "site: $dir/out/current, log: $dir/dashboard.log"
	;;
uninstall)
	unload
	rm -f "$plist"
	echo "uninstalled $label (its files, if any, stay in $dir)"
	;;
status)
	domains=$(loaded)
	if [ -z "$domains" ]; then
		echo "$label not loaded"
	fi
	for d in $domains; do
		echo "$d/$label:"
		launchctl print "$d/$label" | grep -E '^	(state|pid|runs|last exit code) =' || true
	done
	;;
*) usage ;;
esac
