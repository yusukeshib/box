#!/usr/bin/env bash
# Standalone regression suite: bash tests/completions.sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/sessions/alpha" "$fixture/sessions/beta"
touch "$fixture/sessions/alpha/repos" "$fixture/sessions/beta/repos"
export BOX_ROOT="$fixture"
export BOX_COMPLETION_ROOT="$root"
for shell in bash zsh; do
    "$shell" "$root/tests/completions-cases.sh"
done
