#!/bin/sh
# Deterministic fault controls for the fuzz targets (docs/PLAN-improvements.md, I3), run by
# `make fuzz-control` from the repository root.
#
# Each control is `<name>.patch` (one planted bug in the code under test) and `<name>.input`
# (a committed input that exposes it), for the target named in CONTROLS below. In a scratch
# copy of the working tree (target/fuzz-control/run, not a git worktree), every input must
# pass on the unpatched code, then crash its target once its patch is applied. A run on a
# given input is deterministic: the target executes that one input, nothing is mutated.
#
# FUZZ_TOOLCHAIN selects the nightly toolchain (default `nightly`; CI pins a dated one).
set -u

CONTROLS="pyth_partial_accepted:pyth_update
swap_long_collateral_rounds_down:swap_conservation
window_writes_before_refill_check:window_sample
program_skips_confidence:sample_ix"

root=$(pwd)
tc=${FUZZ_TOOLCHAIN:-nightly}
work=$root/target/fuzz-control/run
rm -rf "$work"
mkdir -p "$work"
# The working tree's tracked and untracked (not ignored) files, so uncommitted edits count.
git ls-files -co --exclude-standard | tar -cf - -T - | tar -xf - -C "$work" || exit 1
cd "$work" || exit 1

program() {
    SBF_ARCH="${SBF_ARCH:-v3}" sh scripts/sbf-build.sh program/Cargo.toml >/dev/null 2>&1 ||
        { echo "fuzz-control: program build failed"; exit 1; }
}

# One input through its target: exit status 0 = no crash.
run_input() {
    cargo "+$tc" fuzz run -O "$1" "$2" >"$3" 2>&1
}

program
fail=0
for pair in $CONTROLS; do
    name=${pair%%:*}
    target=${pair##*:}
    input=$root/fuzz/controls/$name.input
    patchfile=$root/fuzz/controls/$name.patch
    log=$work/$name

    if ! run_input "$target" "$input" "$log.clean.log"; then
        echo "fuzz-control: $name: the input crashes $target without the patch"
        tail -20 "$log.clean.log"
        fail=1
        continue
    fi
    patch -s -p1 <"$patchfile" || { echo "fuzz-control: $name: patch does not apply"; exit 1; }
    [ "$target" = sample_ix ] && program
    if run_input "$target" "$input" "$log.patched.log"; then
        echo "fuzz-control: $name: $target did not crash with the planted bug"
        fail=1
    elif ! grep -q "panicked at" "$log.patched.log"; then
        echo "fuzz-control: $name: $target failed without a panic (build error?)"
        tail -20 "$log.patched.log"
        fail=1
    else
        echo "fuzz-control: $name: $target crashes with the planted bug, passes without it"
    fi
    patch -s -R -p1 <"$patchfile" || { echo "fuzz-control: $name: patch does not revert"; exit 1; }
    [ "$target" = sample_ix ] && program
done
exit $fail
