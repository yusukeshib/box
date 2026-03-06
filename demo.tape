# VHS demo tape for box
# Run: vhs demo.tape
# Prerequisites: inside a git repo with box installed

Output demo.gif

Set Shell "bash"
Set FontSize 14
Set Width 1100
Set Height 700
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
Type 'eval "$(box config bash)"'
Enter
Sleep 500ms
Type "box repo add ."
Enter
Sleep 1000ms
Type "clear"
Enter
Sleep 500ms
Show

# ── 1. Create a new session ──────────────────────────────────

Sleep 500ms
Type "box new my-feature --repo box"
Sleep 500ms
Enter

Sleep 3000ms

# ── 2. Explore the isolated workspace ────────────────────────

Type "ls"
Sleep 200ms
Enter
Sleep 1500ms

Type "git log --oneline -5"
Sleep 200ms
Enter
Sleep 1500ms

Type "git status"
Sleep 200ms
Enter
Sleep 1500ms

# ── 3. List sessions ─────────────────────────────────────────

Type "box list"
Sleep 200ms
Enter
Sleep 2000ms

# ── 4. Clean up ──────────────────────────────────────────────

Type "box remove my-feature"
Sleep 200ms
Enter
Sleep 2000ms

Sleep 3s
