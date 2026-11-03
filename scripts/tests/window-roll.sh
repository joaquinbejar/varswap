#!/bin/sh
# Offline test of scripts/window-roll.sh `run`: a mock varswap-admin, mock agent scripts and a
# fake launchctl, with HOME in a scratch directory, so no RPC, no transaction and no real
# LaunchAgent is touched.
#
#   sh scripts/tests/window-roll.sh
#
# The mock admin derives the address of the window starting at S as "W<S>" and keeps each
# window's state in $t/state/<address>: no file is "not found", the text "rpc-fail" is an RPC
# error, anything else is the state. Every create and agent install is appended to $t/calls.

set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
roll=$root/scripts/window-roll.sh
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin" "$t/scripts" "$t/state" "$t/home"

cat >"$t/bin/varswap-admin" <<'EOF'
#!/bin/sh
t=$MOCK_DIR
case $1 in
window-address)
	while [ $# -gt 0 ]; do
		[ "$1" = --start ] && echo "W$2"
		shift
	done
	;;
show-window)
	f=$t/state/$2
	if [ ! -f "$f" ]; then
		echo "Error: account $2 not found"
		exit 1
	fi
	if [ "$(cat "$f")" = rpc-fail ]; then
		echo "Error: AccountNotFound: pubkey=$2: error sending request"
		exit 1
	fi
	printf '{\n  "state": "%s",\n}\n' "$(cat "$f")"
	;;
create-window)
	while [ $# -gt 0 ]; do
		[ "$1" = --start ] && s=$2
		shift
	done
	echo "create $s" >>"$t/calls"
	echo pending >"$t/state/W$s"
	echo "window W$s"
	echo "confirmed SIG$s"
	;;
*) exit 2 ;;
esac
EOF

# keeper-agent.sh and watch-agent.sh: record the install, write a plist listing the windows and
# mark the agent loaded for the fake launchctl.
for kind in keeper watch; do
	cat >"$t/scripts/$kind-agent.sh" <<EOF
#!/bin/sh
echo "$kind \$*" >>"\$MOCK_DIR/calls"
name=\$2
shift 2
mkdir -p "\$HOME/Library/LaunchAgents"
echo "\$*" >"\$HOME/Library/LaunchAgents/com.varswap.$kind.\$name.plist"
echo "com.varswap.$kind.\$name" >>"\$MOCK_DIR/loaded"
EOF
done

cat >"$t/bin/launchctl" <<'EOF'
#!/bin/sh
[ "$1" = print ] || exit 1
label=${2##*/}
[ -f "$MOCK_DIR/loaded" ] && grep -qx "$label" "$MOCK_DIR/loaded"
EOF
chmod +x "$t/bin/varswap-admin" "$t/bin/launchctl"

# The same anchors as window-roll.sh, at 17:00Z.
day=86400
now=$(date -u +%s)
next=$((now - now % day + 17 * 3600))
[ "$next" -gt "$now" ] || next=$((next + day))
old=$((next - 2 * day))
cur=$((next - day))

fail=0
check() {
	if [ "$2" = "$3" ]; then
		echo "ok: $1"
	else
		echo "FAIL: $1: expected [$3], got [$2]"
		fail=1
	fi
}

# A clean state: $1 to $3 are the states of the old, current and next windows ("" is missing).
reset() {
	rm -rf "${t:?}/state" "${t:?}/home" "${t:?}/calls" "${t:?}/loaded"
	mkdir -p "$t/state" "$t/home"
	: >"$t/calls"
	[ -z "$1" ] || echo "$1" >"$t/state/W$old"
	[ -z "$2" ] || echo "$2" >"$t/state/W$cur"
	[ -z "$3" ] || echo "$3" >"$t/state/W$next"
}

# Runs window-roll.sh run; prints its exit code; its output goes to $t/out.
roll() {
	set +e
	env -i PATH="$t/bin:/usr/bin:/bin" HOME="$t/home" MOCK_DIR="$t" \
		SOLANA_URL=http://127.0.0.1:1 VARSWAP_PROGRAM_ID=P VARSWAP_KEYPAIR=/dev/null \
		VARSWAP_ROLL_FEED=ab VARSWAP_ADMIN_BIN="$t/bin/varswap-admin" VARSWAP_ROLL_SCRIPTS="$t/scripts" \
		VARSWAP_ROLL_KEEPER=k VARSWAP_ROLL_KEEPER_KEYPAIR=/dev/null VARSWAP_ROLL_WATCH=w \
		VARSWAP_ROLL_DRY_RUN="${DRY:-0}" sh "$roll" run >"$t/out" 2>&1
	echo $?
	set -e
}

calls() { tr '\n' ';' <"$t/calls"; }

# The next window is missing: created once, and both agents get the open windows.
reset finalized pending ""
check "missing next window: exit" "$(roll)" 0
check "missing next window: calls" "$(calls)" "create $next;keeper install k W$cur W$next;watch install w W$cur W$next;"

# A second run changes nothing.
: >"$t/calls"
check "rerun: exit" "$(roll)" 0
check "rerun: no create, no reinstall" "$(calls)" ""

# A read error on any of the three windows stops the run before anything is sent.
for at in next old cur; do
	case $at in
	next) reset finalized pending rpc-fail ;;
	old) reset rpc-fail pending "" ;;
	cur) reset finalized rpc-fail "" ;;
	esac
	check "read error on the $at window: exit" "$(roll)" 1
	check "read error on the $at window: nothing sent" "$(calls)" ""
	check "read error on the $at window: logged" \
		"$(grep -c 'failed, nothing changed' "$t/out")" 1
done

# A dry run reports the create and the reinstalls, and sends nothing.
reset finalized pending ""
check "dry run: exit" "$(DRY=1 roll)" 0
check "dry run: nothing sent" "$(calls)" ""
check "dry run: would create" "$(grep -c "would create W$next" "$t/out")" 1
check "dry run: would reinstall both" "$(grep -c 'would reinstall' "$t/out")" 2

# Yesterday's window still open is kept; a finalized next window is not.
reset pending pending finalized
check "open old window: exit" "$(roll)" 0
check "open old window: calls" "$(calls)" "keeper install k W$old W$cur;watch install w W$old W$cur;"

exit $fail
