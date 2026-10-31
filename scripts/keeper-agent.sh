#!/bin/sh
# Runs varswap-keeper under launchd (macOS) so it survives reboots and crashes.
#
#   scripts/keeper-agent.sh install <name> <window> [<window> ...]
#   scripts/keeper-agent.sh uninstall <name>
#   scripts/keeper-agent.sh status <name>
#
# `install` copies target/release/varswap-keeper (run `cargo build --release -p varswap-keeper`
# first; VARSWAP_KEEPER_BIN overrides the path) to $VARSWAP_KEEPER_HOME/<name>/ (default ~/.local/share/varswap-keeper), writes the
# LaunchAgent ~/Library/LaunchAgents/com.varswap.keeper.<name>.plist and loads it. The agent
# starts now and at every login (RunAtLoad), and is restarted if it exits with an error or is killed; it is
# not restarted after a clean exit, which `--watch` makes only once every window is final.
#
# Configuration: SOLANA_URL, VARSWAP_PROGRAM_ID and VARSWAP_KEYPAIR are taken from the current
# environment and written into the plist, so none of them may hold a secret (use a public RPC
# URL, or export a keyed one in ~/.zshrc and leave SOLANA_URL unset here). The Hermes key is
# never written anywhere: the agent runs `zsh -c 'source ~/.zshrc; exec varswap-keeper ...'`,
# so VARSWAP_HERMES_API_KEY comes from ~/.zshrc at each start. Signatures go to
# <home>/<name>/keeper.sigs and the report lines to <home>/<name>/keeper.log.
#
# Run one agent per payer keypair (pyth-post's leftover scan assumes it).

set -eu

usage() {
	sed -n '4,6p' "$0" | sed 's/^# *//' >&2
	exit 2
}

[ $# -ge 2 ] || usage
cmd=$1
name=$2
shift 2
case $name in
*[!A-Za-z0-9_-]* | '') echo "name must be letters, digits, - or _" >&2 && exit 2 ;;
esac

home=${VARSWAP_KEEPER_HOME:-$HOME/.local/share/varswap-keeper}
dir=$home/$name
label=com.varswap.keeper.$name
plist=$HOME/Library/LaunchAgents/$label.plist
# The login session's domain when this runs inside it, otherwise (an SSH or background shell)
# the per-user domain. Either way the plist in ~/Library/LaunchAgents is loaded again into the
# login session at every login, so the agent comes back after a reboot.
if launchctl print "gui/$(id -u)" >/dev/null 2>&1; then
	domain=gui/$(id -u)
else
	domain=user/$(id -u)
fi

xml() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }

case $cmd in
install)
	[ $# -ge 1 ] || usage
	: "${SOLANA_URL:?set SOLANA_URL}" "${VARSWAP_PROGRAM_ID:?set VARSWAP_PROGRAM_ID}" \
		"${VARSWAP_KEYPAIR:?set VARSWAP_KEYPAIR}"
	root=$(cd "$(dirname "$0")/.." && pwd)
	bin=${VARSWAP_KEEPER_BIN:-$root/target/release/varswap-keeper}
	[ -x "$bin" ] || { echo "$bin missing: cargo build --release -p varswap-keeper" >&2 && exit 1; }
	for w in "$@"; do
		case $w in *[!1-9A-HJ-NP-Za-km-z]*) echo "not a base58 address: $w" >&2 && exit 2 ;; esac
	done
	mkdir -p "$dir" "$HOME/Library/LaunchAgents"
	cp "$bin" "$dir/varswap-keeper"
	# zsh word-splits nothing here: every window was checked to be base58 above.
	run="source \"\$HOME/.zshrc\" >/dev/null 2>&1; exec \"$dir/varswap-keeper\" --watch 300 $*"
	cat >"$plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key><string>$label</string>
	<key>ProgramArguments</key>
	<array>
		<string>/bin/zsh</string>
		<string>-c</string>
		<string>$(xml "$run")</string>
	</array>
	<key>EnvironmentVariables</key>
	<dict>
		<key>SOLANA_URL</key><string>$(xml "$SOLANA_URL")</string>
		<key>VARSWAP_PROGRAM_ID</key><string>$(xml "$VARSWAP_PROGRAM_ID")</string>
		<key>VARSWAP_KEYPAIR</key><string>$(xml "$VARSWAP_KEYPAIR")</string>
	</dict>
	<!-- Aqua: the login session, at every login. Background: loaded now from an SSH or
	     background shell (the user domain), as the Studio's agents are. -->
	<key>LimitLoadToSessionType</key>
	<array>
		<string>Aqua</string>
		<string>Background</string>
	</array>
	<key>RunAtLoad</key><true/>
	<key>KeepAlive</key>
	<dict>
		<key>SuccessfulExit</key><false/>
	</dict>
	<key>ThrottleInterval</key><integer>60</integer>
	<key>StandardOutPath</key><string>$(xml "$dir/keeper.sigs")</string>
	<key>StandardErrorPath</key><string>$(xml "$dir/keeper.log")</string>
	<key>WorkingDirectory</key><string>$(xml "$dir")</string>
</dict>
</plist>
EOF
	plutil -lint "$plist" >/dev/null
	launchctl bootout "$domain/$label" 2>/dev/null || true
	launchctl bootstrap "$domain" "$plist"
	echo "installed $label: $plist"
	echo "log: $dir/keeper.log, signatures: $dir/keeper.sigs"
	;;
uninstall)
	launchctl bootout "$domain/$label" 2>/dev/null || true
	rm -f "$plist"
	echo "uninstalled $label (its files, if any, stay in $dir)"
	;;
status)
	launchctl print "$domain/$label" 2>/dev/null | grep -E '^\s*(state|pid|last exit code) =' || echo "$label not loaded"
	;;
*) usage ;;
esac
