#!/bin/sh
# Cross-compiles a Windows test .exe from macOS and zips it with the test guide.
# Needs: rustup target add x86_64-pc-windows-gnu && brew install mingw-w64
#
# Writes to dist/test/ so it never replaces dist/RAMOptimizer.exe, the signed
# release build that scripts/release.sh downloads from GitHub Actions.
set -e
cd "$(dirname "$0")/.."
zip="RAMOptimizer-test-windows.zip"
cargo build --release --target x86_64-pc-windows-gnu
mkdir -p dist/test
cp target/x86_64-pc-windows-gnu/release/ram-optimizer.exe dist/test/RAMOptimizer.exe
cp dist/HOW-TO-TEST.txt dist/test/
rm -f "dist/$zip"
(cd dist/test && zip -q "../$zip" RAMOptimizer.exe HOW-TO-TEST.txt)
echo "Built dist/$zip"
