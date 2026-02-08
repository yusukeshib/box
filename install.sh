#!/bin/bash
# realm installer
# Usage: curl -fsSL https://raw.githubusercontent.com/yusukeshib/realm/main/install.sh | bash

set -e

INSTALL_DIR="${HOME}/.realm"
BIN_DIR="$HOME/.local/bin"
BASE_URL="https://raw.githubusercontent.com/yusukeshib/realm/main"

echo "Installing realm..."

if ! command -v docker &>/dev/null; then
    echo "Error: docker is not installed. See https://docs.docker.com/get-docker/" >&2
    exit 1
fi
if ! docker info &>/dev/null; then
    echo "Error: Docker daemon is not running. Please start Docker." >&2
    exit 1
fi

mkdir -p "$INSTALL_DIR"
echo "Downloading realm..."
curl -fsSL "$BASE_URL/realm" -o "$INSTALL_DIR/realm"
chmod +x "$INSTALL_DIR/realm"

mkdir -p "$BIN_DIR"
ln -sf "$INSTALL_DIR/realm" "$BIN_DIR/realm"
echo "Installed to $BIN_DIR/realm"

echo ""
echo "Done! Make sure ~/.local/bin is in your PATH:"
echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
echo ""
echo "Try it out:"
echo "  cd ~/your-git-repo && realm new my-session"
echo "  realm new my-session --image ubuntu:latest -- bash"
echo ""
