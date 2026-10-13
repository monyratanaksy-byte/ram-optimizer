#!/bin/sh
# Cross-compiles the Windows .exe from macOS and zips it with the test guide.
# Needs: rustup target add x86_64-pc-windows-gnu && brew install mingw-w64
set -e
cd "$(dirname "$0")/.."
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
zip="RAMOptimizer-v$version-windows.zip"
cargo build --release --target x86_64-pc-windows-gnu
cp target/x86_64-pc-windows-gnu/release/ram-optimizer.exe dist/RAMOptimizer.exe
rm -f dist/RAMOptimizer*-windows.zip
(cd dist && zip -q "$zip" RAMOptimizer.exe HOW-TO-TEST.txt)
echo "Built dist/$zip"
