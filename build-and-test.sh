#!/bin/sh

set -e
cd litterbox

echo "Building"
cargo build

echo "Running Tests"
cargo test

echo "Checking Formatting"
cargo fmt --check

echo "Linting Code"
cargo clippy
