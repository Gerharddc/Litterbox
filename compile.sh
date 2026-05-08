#!/bin/sh
set -e

HOST_ARCH="$(rustc -vV | sed -n 's/^host: //p' | cut -d- -f1)"
INIT_TARGET="${HOST_ARCH}-unknown-linux-musl"

if [ "${1:-}" = "--release" ]; then
    CARGO_FLAGS="--release"
    OUT_DIR="release"
else
    CARGO_FLAGS=""
    OUT_DIR="debug"
fi

check_target() {
    if ! rustup target list --installed | grep -q "^$1$"; then
        echo "Installing target $1..."
        rustup target add "$1"
    fi
}

echo "=== Building litterbox ==="
cd litterbox
cargo build $CARGO_FLAGS
cd ..

echo "=== Building lbx-init (target: $INIT_TARGET) ==="
check_target "$INIT_TARGET"
cd lbx-init
cargo build $CARGO_FLAGS --target "$INIT_TARGET"
cd ..

echo "=== Copying lbx-init next to litterbox ==="
cp -v "lbx-init/target/$INIT_TARGET/$OUT_DIR/lbx-init" "litterbox/target/$OUT_DIR/"

echo ""
echo "Build complete:"
ls -lh "litterbox/target/$OUT_DIR/litterbox" "litterbox/target/$OUT_DIR/lbx-init"
