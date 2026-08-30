#!/bin/bash
# Reclaims build artifacts nothing has asked for in a while.
#
# Cargo never reclaims what it supersedes — a fingerprint that stops matching
# is simply left on disk — so a `target/` grows without bound across rebuilds
# even when nothing else changes. One here reached 75G that way, and there is
# one per worktree.
#
# Nothing below is specific to this repo: it takes roots and sweeps whatever
# Cargo projects it finds under them, so a second Rust checkout is covered by
# adding a path rather than by copying this file.
#
# Usage: sweep.sh [days] [root...]      (default: 30 days, this repo)
#
# `--hidden` is the flag that matters: cargo-sweep's `--recursive` skips
# directories starting with a dot, and this repo's worktrees live under
# `.claude/worktrees/`, so without it the sweep walks straight past the copies
# that caused the problem and cleans only the main checkout.
#
# Deleting is cheap to be wrong about — sccache means a swept artifact is
# recompiled from cache rather than from source. See ~/.cargo/config.toml.
set -uo pipefail

DAYS="${1:-30}"
shift 2>/dev/null
ROOTS=("$@")
[ ${#ROOTS[@]} -eq 0 ] && ROOTS=("$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)")

command -v cargo >/dev/null 2>&1 || {
    echo "cargo not on PATH" >&2
    exit 1
}
# Asked through cargo, not looked for on PATH: cargo finds `cargo-*`
# subcommands in its own bin directory whether or not that is on PATH, so
# `command -v cargo-sweep` says "missing" on a machine where it works fine.
cargo sweep --version >/dev/null 2>&1 || {
    echo "cargo-sweep not installed: cargo install cargo-sweep" >&2
    exit 1
}

# Roots that do not exist are skipped rather than fatal: a machine is entitled
# to be missing one of the checkouts a shared invocation names.
present=()
for r in "${ROOTS[@]}"; do
    if [ -d "$r" ]; then present+=("$r"); else echo "skipping absent root: $r" >&2; fi
done
[ ${#present[@]} -eq 0 ] && { echo "no roots to sweep" >&2; exit 1; }

kb() { du -sk "$@" 2>/dev/null | awk '{t+=$1} END {print t+0}'; }
gb() { awk -v k="$1" 'BEGIN {printf "%.1fG", k/1048576}'; }

before=$(kb "${present[@]}")

# A sweep that fails must not be reported as a sweep that found nothing: this
# ran under launchd printing "0.0G reclaimed" because cargo was off the agent's
# PATH, which reads exactly like a tidy checkout.
if ! cargo sweep --recursive --hidden --time "$DAYS" "${present[@]}"; then
    echo "cargo-sweep failed; nothing reclaimed" >&2
    exit 1
fi

after=$(kb "${present[@]}")
printf 'swept artifacts older than %s days from %s: %s -> %s (%s reclaimed)\n' \
    "$DAYS" "${present[*]}" "$(gb "$before")" "$(gb "$after")" \
    "$(gb "$((before - after))")"
