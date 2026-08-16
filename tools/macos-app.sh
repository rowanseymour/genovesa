#!/usr/bin/env bash
# Builds target/Genovesa.app, the game as a macOS application.
#
#   tools/macos-app.sh          # build it
#   tools/macos-app.sh --open   # and hand it to Finder afterwards
#
# A bundle is a directory with a fixed shape, and the shape is the whole of
# what makes a binary into an application: an icon in the dock, a name in the
# menu bar, a thing that can be double-clicked. There is no compilation here
# beyond the release build — everything below is copying files into the places
# macOS looks for them.
#
# What this does not do is make an application anyone else can open. The
# signature is ad-hoc, which is enough for the machine that built it and no
# further: Gatekeeper stops an ad-hoc bundle that arrived from somewhere else,
# and getting past that needs a Developer ID certificate, notarisation and a
# stapled ticket. That is a subscription and an account rather than a script,
# so it stays out of one.
set -euo pipefail

me=$(basename "$0")
root=$(cd "$(dirname "$0")/.." && pwd)

open_after=false
case "${1-}" in
    --open) open_after=true ;;
    "") ;;
    *)
        echo "usage: $me [--open]" >&2
        exit 2
        ;;
esac

if [[ $(uname) != Darwin ]]; then
    echo "$me: this builds a macOS bundle, and this is not a Mac" >&2
    exit 1
fi

# The name is the application's, not the crate's. `game` is what the binary is
# called because that is what it is amongst the other binaries in the
# workspace; on a dock it has to be the name of the thing.
NAME=Genovesa
# Reverse DNS, and it has to be unique rather than meaningful: it is the handle
# Launch Services files the application under, and two applications sharing one
# is how a machine ends up opening the wrong one.
IDENT=io.github.rowanseymour.genovesa
# Rust's own floor for aarch64-apple-darwin, so claiming lower would be a lie
# about a binary that would not launch there anyway.
MINIMUM_MACOS=11.0

# The version lives in the crate manifest and is read from it, so that a bundle
# cannot claim a version the build does not have.
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/crates/game/Cargo.toml" | head -1)
if [[ -z $version ]]; then
    echo "$me: no version in crates/game/Cargo.toml" >&2
    exit 1
fi

# Built without `dev`: that feature's file watcher is a thread watching the
# assets directory for edits, and a bundle's assets do not change under it.
cargo build --release --quiet --manifest-path "$root/Cargo.toml" -p game --bin game

# Staged beside where it is going rather than in /tmp, so the last step is a
# rename within one filesystem and a killed run cannot leave half an
# application where a whole one was.
app=$root/target/$NAME.app
staged=$(mktemp -d "$root/target/.$NAME.app.XXXXXX")
trap 'rm -rf "$staged"' EXIT

contents=$staged/Contents
mkdir -p "$contents/MacOS" "$contents/Resources"

# The three things a bundle is: the executable, everything it reads, and the
# plist that tells macOS which is which.
cp "$root/target/release/game" "$contents/MacOS/$NAME"
# A release build carries symbols it has no use for once it is an application —
# a fifth of the file, and the first thing anyone would ask why they were
# sending. Before the signature rather than after, since stripping a binary
# invalidates one.
strip -x "$contents/MacOS/$NAME"

# Resources, because that is where macOS keeps everything a program only reads
# — and `asset_plugin` in the game is the other half of it. Bevy would look
# beside the binary otherwise, which is one directory over and empty.
cp -R "$root/assets" "$contents/Resources/assets"
"$root/assets-src/icon/build.py" --icns "$contents/Resources/$NAME.icns" >/dev/null

cat >"$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleName</key>
	<string>$NAME</string>
	<key>CFBundleIdentifier</key>
	<string>$IDENT</string>
	<key>CFBundleExecutable</key>
	<string>$NAME</string>
	<key>CFBundleIconFile</key>
	<string>$NAME</string>
	<key>CFBundleShortVersionString</key>
	<string>$version</string>
	<key>CFBundleVersion</key>
	<string>$version</string>
	<key>LSMinimumSystemVersion</key>
	<string>$MINIMUM_MACOS</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.games</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

# Ad-hoc, which seals the bundle so that the system treats it as one
# application rather than a directory that happens to contain a binary. See the
# note at the top about what this is not.
codesign --force --sign - "$staged" >/dev/null 2>&1

rm -rf "$app"
mv "$staged" "$app"
trap - EXIT

# Finder caches an icon against the bundle it came from and will keep showing
# the old one — or none — until the directory's date moves.
touch "$app"

echo "$me: $app ($NAME $version, $(du -sh "$app" | cut -f1))"

if [[ $open_after == true ]]; then
    open -R "$app"
fi
