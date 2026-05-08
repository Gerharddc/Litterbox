#!/usr/bin/env bash
set -e

INSTALL_DIR="$HOME/.local/bin"
BINARIES=("litterbox" "lbx-init")

print_path_warning() {
  case ":$PATH:" in
    *":$HOME/.local/bin:"*)
      echo "✓ $HOME/.local/bin is already in your PATH."
      ;;
    *)
      echo "⚠️  NOTE: $HOME/.local/bin is not in your PATH."
      echo "Add this to your shell config (e.g., ~/.bashrc or ~/.zshrc):"
      echo ""
      echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
      echo ""
      ;;
  esac
}

uninstall() {
  echo ""
  echo "=== Uninstalling Litterbox ==="
  echo ""

  for bin in "${BINARIES[@]}"; do
    TARGET="$INSTALL_DIR/$bin"
    if [[ -f "$TARGET" ]]; then
      echo "Removing $TARGET"
      rm -f "$TARGET"
    else
      echo "Not found: $TARGET"
    fi
  done

  echo ""
  echo "Uninstallation complete!"
  echo ""
  exit 0
}

if [[ "$1" == "--uninstall" ]]; then
  uninstall
fi

echo ""
echo "=== Installing Litterbox ==="
echo ""

# Find the embedded archive
PAYLOAD_LINE=$(grep -n "^__ARCHIVE_BELOW__$" "$0" | cut -d: -f1)
PAYLOAD_LINE=$((PAYLOAD_LINE + 1))

TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

echo "Extracting embedded package..."
tail -n +$PAYLOAD_LINE "$0" | tar -xz -C "$TMPDIR"

mkdir -p "$INSTALL_DIR"

echo "Installing binaries to $INSTALL_DIR ..."
for bin in "${BINARIES[@]}"; do
  install -m 755 "$TMPDIR/$bin" "$INSTALL_DIR"
done

echo ""
echo "Installation complete!"
echo ""

print_path_warning

exit 0

__ARCHIVE_BELOW__
