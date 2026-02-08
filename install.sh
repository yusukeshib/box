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

# Download realm.sh (contains embedded Dockerfile and entrypoint.sh)
mkdir -p "$INSTALL_DIR"
echo "Downloading realm.sh..."
curl -fsSL "$BASE_URL/realm.sh" -o "$INSTALL_DIR/realm.sh"
chmod +x "$INSTALL_DIR/realm.sh"

# Note: The Docker image will be built automatically on first run
# This allows realm.sh to manage the build and fingerprinting itself

# Install to ~/.local/bin
mkdir -p "$BIN_DIR"
ln -sf "$INSTALL_DIR/realm.sh" "$BIN_DIR/realm"
echo "Installed to $BIN_DIR/realm"

echo ""
echo "✓ Realm installed successfully!"
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  IMPORTANT: Set up authentication"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "On macOS, add your API key to your shell config:"
echo "  echo 'export ANTHROPIC_API_KEY=\"sk-ant-...\"' >> ~/.zshrc"
echo "  source ~/.zshrc"
echo ""
echo "Get your API key: https://console.anthropic.com"
echo ""
echo "Then try: realm new my-session ~/projects/my-app"
echo ""
