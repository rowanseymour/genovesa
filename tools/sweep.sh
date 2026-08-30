#!/bin/bash
# Reclaims build artifacts nothing has asked for in a while.
#
# A `target/` here is 2.7G the day it is built, and cargo never reclaims what
# it supersedes — a fingerprint that stops matching is simply left on disk, so
# the directory grows without bound across rebuilds. One reached 75G that way.
# Multiply by a worktree per feature branch and the checkout outweighs
# everything else on the machine.
#
# `--hidden` is the flag that matters: cargo-sweep's `--recursive` skips
# directories starting with a dot, and the worktrees live under
# `.claude/worktrees/`, so without it this would walk straight past the copies
# that caused the problem and only ever clean the main checkout.
#
# Deleting is cheap to be wrong about — sccache means a swept artifact is
# recompiled from cache rather than from source. See ~/.cargo/config.toml.
set -uo pipefail

DAYS="${1:-30}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

command -v cargo >/dev/null 2>&1 || {
    echo "cargo not on PATH" >&2
    exit 1
}
command -v cargo-sweep >/dev/null 2>&1 || {
    echo "cargo-sweep not installed: cargo install cargo-sweep" >&2
    exit 1
}

before=$(du -sk "$ROOT" 2>/dev/null | cut -f1)

# Worktrees git has forgotten leave their target/ behind with nothing to
# rebuild it for. Prune first so the sweep sees the directories as orphans.
git -C "$ROOT" worktree prune 2>/dev/null

# A sweep that fails must not be reported as a sweep that found nothing: this
# ran for a week under launchd printing "0.0G reclaimed" because cargo was off
# the agent's PATH, which reads exactly like a tidy checkout.
if ! cargo sweep --recursive --hidden --time "$DAYS" "$ROOT"; then
    echo "cargo-sweep failed; nothing reclaimed" >&2
    exit 1
fi

after=$(du -sk "$ROOT" 2>/dev/null | cut -f1)
printf 'swept artifacts older than %s days: %s -> %s (%s reclaimed)\n' \
    "$DAYS" \
    "$(echo "$before" | awk '{printf "%.1fG", $1/1048576}')" \
    "$(echo "$after"  | awk '{printf "%.1fG", $1/1048576}')" \
    "$(echo "$before $after" | awk '{printf "%.1fG", ($1-$2)/1048576}')"
