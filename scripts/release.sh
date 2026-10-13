#!/bin/sh
# Publishes a new version: every installed copy updates itself within hours.
#
#   ./scripts/release.sh 0.2.0 "What changed"
#
# Builds the Windows .exe, signs it with the key in
# ~/.config/ram-optimizer/update-signing.key, commits, tags, pushes, and
# creates the GitHub release the app downloads from.
set -e
cd "$(dirname "$0")/.."

version="$1"
notes="${2:-Bug fixes and improvements.}"
if ! echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "usage: $0 <version like 0.2.0> [release notes]" >&2
    exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
    echo "Commit or stash your changes first; a release must match the code it was built from." >&2
    exit 1
fi
current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
echo "Releasing $current -> $version"

# The app compares this version with the latest release to decide whether to update.
sed -i '' "s/^version = \"$current\"/version = \"$version\"/" Cargo.toml
cargo build --release --target x86_64-pc-windows-gnu
cp target/x86_64-pc-windows-gnu/release/ram-optimizer.exe dist/RAMOptimizer.exe
cargo run -q -p release-tool -- sign dist/RAMOptimizer.exe "$version"
rm -f dist/RAMOptimizer-windows.zip
(cd dist && zip -q RAMOptimizer-windows.zip RAMOptimizer.exe HOW-TO-TEST.txt)

git add Cargo.toml Cargo.lock
git commit -m "Release v$version"
git tag "v$version"
git push
git push origin "v$version"

gh release create "v$version" \
    dist/RAMOptimizer.exe dist/RAMOptimizer.exe.sig dist/RAMOptimizer-windows.zip \
    --title "v$version" --notes "$notes"
echo "Released v$version. Installed copies will update within a few hours (or on their next start)."
