#!/bin/sh
# Publishes a new version: every installed copy updates itself.
#
#   ./scripts/release.sh 0.2.2 "What changed"
#
# Bumps the version, tags and pushes. GitHub Actions then builds the .exe on
# Windows with Microsoft's compiler (.github/workflows/windows.yml); this
# script downloads it, signs it with the key in
# ~/.config/ram-optimizer/update-signing.key and creates the GitHub release
# the app updates from.
set -e
cd "$(dirname "$0")/.."

version="$1"
notes="${2:-Bug fixes and improvements.}"
if ! echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    echo "usage: $0 <version like 0.2.2> [release notes]" >&2
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
cargo update --workspace --offline -q
git add Cargo.toml Cargo.lock
git commit -q -m "Release v$version" ${RELEASE_TRAILER:+-m "$RELEASE_TRAILER"}
git tag "v$version"
git push -q
git push -q origin "v$version"

echo "Waiting for the Windows build on GitHub Actions..."
run_id=""
for _ in $(seq 1 30); do
    run_id=$(gh run list --workflow windows.yml --branch "v$version" --limit 1 --json databaseId --jq '.[0].databaseId')
    [ -n "$run_id" ] && break
    sleep 5
done
if [ -z "$run_id" ]; then
    echo "The Windows build didn't start; check the Actions tab on GitHub." >&2
    exit 1
fi
gh run watch "$run_id" --exit-status --interval 20 > /dev/null

rm -rf dist/ci
gh run download "$run_id" -n RAMOptimizer -D dist/ci
cp dist/ci/ram-optimizer.exe dist/RAMOptimizer.exe
cargo run -q -p release-tool -- sign dist/RAMOptimizer.exe "$version"

zip="RAMOptimizer-v$version-windows.zip"
rm -f dist/RAMOptimizer*-windows.zip
(cd dist && zip -q "$zip" RAMOptimizer.exe HOW-TO-TEST.txt)

# Upload as a draft first, then publish in a separate step: if publishing
# hits a network error, `gh release create` deletes the whole release, but a
# draft survives and the publish step can simply be retried.
gh release create "v$version" \
    dist/RAMOptimizer.exe dist/RAMOptimizer.exe.sig "dist/$zip" \
    --title "v$version" --notes "$notes" --draft --verify-tag
for attempt in 1 2 3 4 5; do
    gh release edit "v$version" --draft=false --latest && break
    if [ "$attempt" = 5 ]; then
        echo "Couldn't publish the draft; run: gh release edit v$version --draft=false --latest" >&2
        exit 1
    fi
    sleep 10
done
echo "Released v$version. Installed copies update the next time they open."
