# VHS demo tape for box
# Run: vhs demo.tape
# Prerequisites: box installed, git repos exist at /tmp/demo-repos/{frontend,backend,api-gateway}

Output demo.gif

Set Shell "bash"
Set FontSize 20
Set Width 900
Set Height 550
Set Padding 20
Set TypingSpeed 60ms

# Set up environment silently
Hide
Type "export PATH=$HOME/.cargo/bin:$PATH"
Enter
Sleep 500ms
Type "export BOX_ROOT=/tmp/box-demo"
Enter
Sleep 500ms
Type "rm -rf /tmp/box-demo /tmp/demo-repos"
Enter
Sleep 500ms
# Create demo git repos
Type "mkdir -p /tmp/demo-repos"
Enter
Sleep 300ms
Type "for r in frontend backend api-gateway; do git init -q /tmp/demo-repos/$r && git -C /tmp/demo-repos/$r commit -q --allow-empty -m 'initial commit'; done"
Enter
Sleep 1000ms
Type 'eval "$(box config bash)"'
Enter
Sleep 500ms
Type "clear"
Enter
Sleep 500ms
Show

# ── 1. Register repos ─────────────────────────────────────

Sleep 500ms
Type "box repo add /tmp/demo-repos/frontend"
Sleep 500ms
Enter
Sleep 1200ms

Type "box repo add /tmp/demo-repos/backend"
Sleep 500ms
Enter
Sleep 1200ms

Type "box repo add /tmp/demo-repos/api-gateway"
Sleep 500ms
Enter
Sleep 1200ms

# ── 2. List registered repos ──────────────────────────────

Type "box repo list"
Sleep 500ms
Enter
Sleep 2000ms

# ── 3. Remove a repo ──────────────────────────────────────

Type "box repo remove api-gateway"
Sleep 500ms
Enter
Sleep 1500ms

Type "box repo list"
Sleep 500ms
Enter
Sleep 2000ms

# ── 4. Launch TUI to create a session ─────────────────────

Type "box"
Sleep 500ms
Enter
Sleep 1500ms

# TUI Stage 1: Select repos (all selected by default)
# Deselect "backend" to show toggling
Down
Sleep 400ms
Space
Sleep 400ms
# Re-select "backend"
Space
Sleep 400ms
Enter
Sleep 1000ms

# TUI Stage 2: Enter session name
Type "my-feature"
Sleep 500ms
Enter
Sleep 1000ms

# TUI Stage 3: Command (optional, skip)
Enter

Sleep 3000ms

# ── 5. Explore the workspace ──────────────────────────────

Type "ls"
Sleep 200ms
Enter
Sleep 1500ms

Type "box list"
Sleep 200ms
Enter
Sleep 2000ms

# ── 6. Clean up ───────────────────────────────────────────

Type "box remove my-feature"
Sleep 200ms
Enter
Sleep 2000ms

Sleep 3s
