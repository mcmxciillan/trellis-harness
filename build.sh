#!/usr/bin/env bash
set -e

echo "============================================================"
echo " Building exo-suit Single-Binary Rust Daemon & PWA (Release)"
echo "============================================================"

cargo build --release

echo "============================================================"
echo " Build successful! Binary details:"
ls -lh target/release/exo-suit
echo ""
echo " Run with: ./target/release/exo-suit"
echo "============================================================"
