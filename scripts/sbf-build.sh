#!/bin/sh
# Builds one crate for SBF and fails if any function's stack frame exceeds 4 KiB.
# cargo build-sbf only prints the overflow (and exits 0); the overflow corrupts locals at
# run time, so rules/global_rules.md makes it a build failure. A failing cargo fails too: its
# status comes from the command substitution, not from a pipe.
# Usage: sbf-build.sh <manifest> [extra cargo build-sbf arguments]
set -u
manifest=$1
shift
out=$(cargo build-sbf --manifest-path "$manifest" --arch "${SBF_ARCH:-v3}" "$@" 2>&1)
status=$?
printf '%s\n' "$out" | grep -v 'installed with the Solana CLI'
if [ $status -ne 0 ]; then
    exit $status
fi
# Current platform-tools wording, and the older "Stack offset of N exceeded max offset".
if printf '%s\n' "$out" | grep -qE 'overflows the maximum allowed frame space|Stack offset of [0-9]+ exceeded'; then
    echo "sbf-build: stack frame over 4 KiB in $manifest"
    exit 1
fi
