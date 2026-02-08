#!/bin/bash
# realm installer
# Usage: curl -fsSL https://raw.githubusercontent.com/yusukeshib/realm/main/install.sh | bash

set -e

if ! command -v cargo &>/dev/null; then
    echo "Error: Rust toolchain is not installed. See https://rustup.rs/" >&2
    exit 1
fi

echo "Installing realm..."
cargo install --git https://github.com/yusukeshib/realm

echo ""
echo "Done! Make sure ~/.cargo/bin is in your PATH:"
echo "  export PATH=\"\$HOME/.cargo/bin:\$PATH\""
echo ""
echo "Try it out:"
echo "  cd ~/your-git-repo && realm switch -c my-session"
echo "  realm switch -c my-session --image ubuntu:latest -- bash"
echo ""
