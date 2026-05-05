#!/bin/sh
set -e

cargo_ok() {
    local crate="$1"
    shift
    echo "  [$crate] cargo $*"
    cd "$crate"
    cargo "$@"
    cd ..
}

build() { cargo_ok "$1" build; }
check() { cargo_ok "$1" fmt --check; }
lint()  { cargo_ok "$1" clippy; }
test_() { cargo_ok "$1" test; }

echo "=== Building ==="
build litterbox
build lbx-init

echo ""
echo "=== Testing ==="
test_ litterbox
test_ lbx-init

echo ""
echo "=== Formatting ==="
check litterbox
check lbx-init

echo ""
echo "=== Linting ==="
lint litterbox
lint lbx-init

echo ""
echo "All checks passed."
