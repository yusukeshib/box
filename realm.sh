#!/bin/bash
set -e

# ============================================================================
# EMBEDDED DOCKERFILE
# ============================================================================
get_dockerfile_content() {
    cat <<'DOCKERFILE_EOF'
FROM node:20-alpine

RUN apk add --no-cache \
    git \
    curl \
    bash \
    ripgrep \
    openssh-client \
    ca-certificates

# Ensure the node user's home directory exists with correct permissions
RUN mkdir -p /home/node && chown -R node:node /home/node

# Install Claude Code globally
ARG CLAUDE_CODE_VERSION=latest
RUN npm install -g @anthropic-ai/claude-code@${CLAUDE_CODE_VERSION}

# Create directories for read-only project mount, writable workspace, and output
RUN mkdir -p /project /workspace /output && chmod 777 /workspace /output

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

WORKDIR /workspace

ENV TERM=xterm-256color
ENV LANG=C.UTF-8
ENV HOME=/home/node

# chmod 777 allows any user (including --user from docker run) to write here
RUN chown -R node:node /home/node && chmod 777 /home/node

ENTRYPOINT ["/entrypoint.sh"]
DOCKERFILE_EOF
}

# ============================================================================
# EMBEDDED ENTRYPOINT
# ============================================================================
get_entrypoint_content() {
    cat <<'ENTRYPOINT_EOF'
#!/bin/bash
set -e

# Copy project to writable workspace
echo "Copying project to workspace..."
cp -r /project/. /workspace/ 2>/dev/null || true
cd /workspace

# Set up git identity and safe directory (container-local only)
git config --global user.email "realm@container"
git config --global user.name "realm"
git config --global safe.directory /workspace

# Create baseline commit capturing current state
if [ ! -d .git ]; then
    git init -q
fi
git add -A
git commit --no-verify --allow-empty -m "realm: baseline" -q
BASELINE=HEAD

# Apply base patch if resuming from a remote branch
if [ -f /output/base.patch ] && [ -s /output/base.patch ]; then
    git apply /output/base.patch
    git add -A
    git commit --no-verify -m "realm: base" -q
fi

# Run Claude (allow non-zero exit, e.g. Ctrl-C)
set +e
claude --dangerously-skip-permissions "$@"
CLAUDE_EXIT=$?
set -e

# Save changes to output
git add -A

if ! git diff --cached --quiet "$BASELINE"; then
    git diff --cached "$BASELINE" > /output/changes.patch
fi

exit $CLAUDE_EXIT
ENTRYPOINT_EOF
}

# ============================================================================
# MAIN SCRIPT
# ============================================================================

# Resolve symlinks to find the actual script directory
SOURCE="$0"
while [ -L "$SOURCE" ]; do
    DIR="$(cd "$(dirname "$SOURCE")" && pwd)"
    SOURCE="$(readlink "$SOURCE")"
    [[ "$SOURCE" != /* ]] && SOURCE="$DIR/$SOURCE"
done
SCRIPT_DIR="$(cd "$(dirname "$SOURCE")" && pwd)"
SESSIONS_DIR="${HOME}/.realm/sessions"

usage() {
    echo "Usage:" >&2
    echo "  realm new <name> [--remote] [project_dir] [claude args...]" >&2
    echo "  realm resume <name> [claude args...]             Resume existing session" >&2
    echo "  realm list                                        List all sessions" >&2
    echo "  realm push <name>                                 Push changes to branch" >&2
    echo "  realm remove <name>                               Delete session" >&2
    echo "  realm upgrade                                     Upgrade to latest version" >&2
    exit 1
}

validate_name() {
    local name="$1"
    if [[ -z "$name" ]]; then
        echo "Error: Session name is required." >&2
        exit 1
    fi
    if [[ ! "$name" =~ ^[a-zA-Z0-9_-]+$ ]]; then
        echo "Error: Invalid session name '${name}'. Use only letters, digits, hyphens, and underscores." >&2
        exit 1
    fi
}

check_docker() {
    if ! command -v docker &>/dev/null; then
        echo "Error: docker is not installed. See https://docs.docker.com/get-docker/" >&2
        exit 1
    fi
    if ! docker info &>/dev/null; then
        echo "Error: Docker daemon is not running. Please start Docker." >&2
        exit 1
    fi
}

check_auth() {
    local has_api_key=false
    local has_credentials_file=false

    if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
        has_api_key=true
    fi

    if [[ -f "${HOME}/.claude/.credentials.json" ]]; then
        has_credentials_file=true
    fi

    # Check if API key is in shell config but not loaded
    local key_in_config=false
    if [[ -f "${HOME}/.zshrc" ]] && grep -q "ANTHROPIC_API_KEY" "${HOME}/.zshrc" 2>/dev/null; then
        key_in_config=true
    elif [[ -f "${HOME}/.bashrc" ]] && grep -q "ANTHROPIC_API_KEY" "${HOME}/.bashrc" 2>/dev/null; then
        key_in_config=true
    fi

    if [[ "$has_api_key" == false ]] && [[ "$has_credentials_file" == false ]]; then
        echo "⚠️  Authentication not configured."
        echo ""

        if [[ "$key_in_config" == true ]]; then
            echo "Found ANTHROPIC_API_KEY in your shell config, but it's not loaded."
            echo "Reload your shell configuration:"
            echo "  source ~/.zshrc  # or source ~/.bashrc"
            echo ""
        else
            echo "Realm needs an Anthropic API key to function."
            echo ""
            echo "On macOS, set your API key in your shell configuration:"
            echo "  echo 'export ANTHROPIC_API_KEY=\"sk-ant-...\"' >> ~/.zshrc"
            echo "  source ~/.zshrc"
            echo ""
            echo "Get your API key from: https://console.anthropic.com"
            echo ""
        fi

        read -p "Continue anyway? (y/N) " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            exit 1
        fi
    fi
}

check_and_rebuild_image() {
    local fingerprint_file="${HOME}/.realm/.build_fingerprint"
    local build_dir="${HOME}/.realm/.build"

    # Compute checksums of embedded content (not files)
    local dockerfile_hash entrypoint_hash current_hash
    dockerfile_hash="$(get_dockerfile_content | shasum -a 256 | awk '{print $1}')"
    entrypoint_hash="$(get_entrypoint_content | shasum -a 256 | awk '{print $1}')"
    current_hash="${dockerfile_hash}:${entrypoint_hash}"

    # Check if rebuild is needed
    local needs_rebuild=false
    local reason=""

    if ! docker image inspect realm:latest &>/dev/null; then
        needs_rebuild=true
        reason="image does not exist"
    elif [[ ! -f "$fingerprint_file" ]]; then
        needs_rebuild=true
        reason="fingerprint file missing"
    else
        local stored_hash
        stored_hash="$(cat "$fingerprint_file")"
        if [[ "$current_hash" != "$stored_hash" ]]; then
            needs_rebuild=true
            reason="build files have changed"
        fi
    fi

    # Rebuild if needed
    if [[ "$needs_rebuild" == true ]]; then
        echo "Rebuilding Docker image ($reason)..."

        # Extract embedded files to build directory
        mkdir -p "$build_dir"
        get_dockerfile_content > "${build_dir}/Dockerfile"
        get_entrypoint_content > "${build_dir}/entrypoint.sh"
        chmod +x "${build_dir}/entrypoint.sh"

        # Build from extracted files
        if docker build --quiet -t realm:latest "$build_dir"; then
            mkdir -p "$(dirname "$fingerprint_file")"
            echo "$current_hash" > "$fingerprint_file"
        else
            echo "Error: Docker build failed." >&2
            exit 1
        fi
    fi
}

cmd_new() {
    local name="$1"
    shift
    validate_name "$name"

    # Parse --remote flag
    local remote=false
    local args=()
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --remote) remote=true; shift ;;
            *) args+=("$1"); shift ;;
        esac
    done
    set -- "${args[@]}"

    # Determine project_dir: if next arg is a directory, use it; otherwise default to "."
    local project_dir="."
    if [[ -n "${1:-}" && -d "$1" ]]; then
        project_dir="$1"
        shift
    fi

    PROJECT_DIR="$(cd "$project_dir" && pwd)"

    if [[ ! -d "$PROJECT_DIR" ]]; then
        echo "Error: Project directory '${project_dir}' does not exist." >&2
        exit 1
    fi

    local session_dir="${SESSIONS_DIR}/${name}"
    if [[ -d "$session_dir" ]]; then
        echo "Error: Session '${name}' already exists. Remove it first: realm remove ${name}" >&2
        exit 1
    fi

    check_docker

    check_and_rebuild_image

    check_auth

    mkdir -p "$session_dir"
    echo "$PROJECT_DIR" > "${session_dir}/project_dir"
    date -u '+%Y-%m-%d %H:%M:%S UTC' > "${session_dir}/created_at"

    # Fetch remote branch and create base patch
    if [[ "$remote" == true ]]; then
        echo "Fetching origin/${name}..."
        git -C "$PROJECT_DIR" fetch origin "$name"
        git -C "$PROJECT_DIR" diff main...origin/"$name" > "${session_dir}/base.patch"
        local base_size
        base_size=$(wc -c < "${session_dir}/base.patch" | tr -d ' ')
        if [[ "$base_size" -eq 0 ]]; then
            rm -f "${session_dir}/base.patch"
            echo "No changes found on origin/${name} relative to main."
        fi
    fi

    mkdir -p "${HOME}/.claude"

    DOCKER_ARGS=(
        --rm -it
        --user "$(id -u):$(id -g)"
        -v "${PROJECT_DIR}:/project:ro"
        -v "${session_dir}:/output"
        -v "${HOME}/.claude:/home/node/.claude"
    )

    if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
        DOCKER_ARGS+=(-e ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY}")
    fi

    if [[ -n "${REALM_DOCKER_ARGS:-}" ]]; then
        eval EXTRA_ARGS=("${REALM_DOCKER_ARGS}")
        DOCKER_ARGS+=("${EXTRA_ARGS[@]}")
    fi

    set +e
    docker run "${DOCKER_ARGS[@]}" realm:latest "$@"
    DOCKER_EXIT=$?
    set -e

    # Clean up empty patch files
    if [[ -f "${session_dir}/changes.patch" ]]; then
        PATCH_SIZE=$(wc -c < "${session_dir}/changes.patch" | tr -d ' ')
        if [[ "$PATCH_SIZE" -eq 0 ]]; then
            rm -f "${session_dir}/changes.patch"
        fi
    fi

    exit $DOCKER_EXIT
}

cmd_resume() {
    local name="$1"
    shift
    validate_name "$name"

    local session_dir="${SESSIONS_DIR}/${name}"
    if [[ ! -d "$session_dir" ]]; then
        echo "Error: Session '${name}' not found." >&2
        exit 1
    fi

    if [[ ! -f "${session_dir}/project_dir" ]]; then
        echo "Error: Session '${name}' is missing project directory metadata." >&2
        exit 1
    fi

    local project_dir
    project_dir="$(cat "${session_dir}/project_dir")"

    if [[ ! -d "$project_dir" ]]; then
        echo "Error: Project directory '${project_dir}' no longer exists." >&2
        exit 1
    fi

    # Check if there are previous changes to resume from
    local has_previous_changes=false
    if [[ -f "${session_dir}/changes.patch" ]]; then
        local patch_size
        patch_size=$(wc -c < "${session_dir}/changes.patch" | tr -d ' ')
        if [[ "$patch_size" -gt 0 ]]; then
            has_previous_changes=true
        fi
    fi

    # Inform user about session state
    if [[ "$has_previous_changes" == true ]]; then
        echo "Resuming session '${name}' with previous changes..."
        # Promote changes.patch to base.patch (cumulative approach)
        if [[ -f "${session_dir}/base.patch" ]]; then
            # Combine existing base.patch with changes.patch
            local tmpfile
            tmpfile="$(mktemp)"
            cat "${session_dir}/base.patch" "${session_dir}/changes.patch" > "$tmpfile"
            mv "$tmpfile" "${session_dir}/base.patch"
        else
            mv "${session_dir}/changes.patch" "${session_dir}/base.patch"
        fi
    else
        echo "Resuming session '${name}'..."
    fi

    # Update resumed_at timestamp
    date -u '+%Y-%m-%d %H:%M:%S UTC' > "${session_dir}/resumed_at"

    check_docker

    check_and_rebuild_image

    check_auth

    mkdir -p "${HOME}/.claude"

    DOCKER_ARGS=(
        --rm -it
        --user "$(id -u):$(id -g)"
        -v "${project_dir}:/project:ro"
        -v "${session_dir}:/output"
        -v "${HOME}/.claude:/home/node/.claude"
    )

    if [[ -n "${ANTHROPIC_API_KEY:-}" ]]; then
        DOCKER_ARGS+=(-e ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY}")
    fi

    if [[ -n "${REALM_DOCKER_ARGS:-}" ]]; then
        eval EXTRA_ARGS=("${REALM_DOCKER_ARGS}")
        DOCKER_ARGS+=("${EXTRA_ARGS[@]}")
    fi

    set +e
    docker run "${DOCKER_ARGS[@]}" realm:latest "$@"
    DOCKER_EXIT=$?
    set -e

    # Clean up empty patch files
    if [[ -f "${session_dir}/changes.patch" ]]; then
        PATCH_SIZE=$(wc -c < "${session_dir}/changes.patch" | tr -d ' ')
        if [[ "$PATCH_SIZE" -eq 0 ]]; then
            rm -f "${session_dir}/changes.patch"
        fi
    fi

    exit $DOCKER_EXIT
}

cmd_list() {
    if [[ ! -d "$SESSIONS_DIR" ]] || [[ -z "$(ls -A "$SESSIONS_DIR" 2>/dev/null)" ]]; then
        echo "No sessions found."
        exit 0
    fi

    printf "%-20s %-40s %s\n" "NAME" "PROJECT" "CREATED"
    printf "%-20s %-40s %s\n" "----" "-------" "-------"

    for session_dir in "${SESSIONS_DIR}"/*/; do
        [[ -d "$session_dir" ]] || continue
        local name
        name="$(basename "$session_dir")"
        local project_dir=""
        local created_at=""

        if [[ -f "${session_dir}/project_dir" ]]; then
            project_dir="$(cat "${session_dir}/project_dir")"
        fi
        if [[ -f "${session_dir}/created_at" ]]; then
            created_at="$(cat "${session_dir}/created_at")"
        fi

        printf "%-20s %-40s %s\n" "$name" "$project_dir" "$created_at"
    done
}

cmd_push() {
    local name="$1"
    validate_name "$name"

    local session_dir="${SESSIONS_DIR}/${name}"
    if [[ ! -d "$session_dir" ]]; then
        echo "Error: Session '${name}' not found." >&2
        exit 1
    fi

    if [[ ! -f "${session_dir}/changes.patch" ]]; then
        echo "Error: No changes in session '${name}'." >&2
        exit 1
    fi

    local patch_size
    patch_size=$(wc -c < "${session_dir}/changes.patch" | tr -d ' ')
    if [[ "$patch_size" -eq 0 ]]; then
        echo "Error: No changes in session '${name}'." >&2
        exit 1
    fi

    if [[ ! -f "${session_dir}/project_dir" ]]; then
        echo "Error: Session '${name}' is missing project directory metadata." >&2
        exit 1
    fi

    local project_dir
    project_dir="$(cat "${session_dir}/project_dir")"

    if [[ ! -d "$project_dir" ]]; then
        echo "Error: Project directory '${project_dir}' no longer exists." >&2
        exit 1
    fi

    # Use a temporary worktree to avoid touching local branches
    local tmpdir
    tmpdir="$(mktemp -d)"

    git -C "$project_dir" worktree add --detach "$tmpdir" HEAD -q
    git -C "$tmpdir" apply "${session_dir}/changes.patch"
    git -C "$tmpdir" add -A
    git -C "$tmpdir" commit -m "realm: ${name}" -q
    git -C "$tmpdir" push --force origin "HEAD:refs/heads/${name}"

    # Clean up worktree
    git -C "$project_dir" worktree remove "$tmpdir"

    echo ""
    echo "Pushed to origin/${name}."
}

cmd_remove() {
    local name="$1"
    validate_name "$name"

    local session_dir="${SESSIONS_DIR}/${name}"
    if [[ ! -d "$session_dir" ]]; then
        echo "Error: Session '${name}' not found." >&2
        exit 1
    fi

    rm -rf "$session_dir"
    echo "Session '${name}' removed."
}

cmd_upgrade() {
    local base_url="https://raw.githubusercontent.com/yusukeshib/realm/main"
    local tmpfile
    tmpfile="$(mktemp)"

    echo "Downloading latest version..."
    if ! curl -fsSL "${base_url}/realm.sh" -o "$tmpfile"; then
        echo "Error: Failed to download latest version." >&2
        rm -f "$tmpfile"
        exit 1
    fi

    # Verify the downloaded file is valid bash
    if ! bash -n "$tmpfile" 2>/dev/null; then
        echo "Error: Downloaded file is not valid." >&2
        rm -f "$tmpfile"
        exit 1
    fi

    # Find the actual script path (resolve symlinks)
    local actual_script="$0"
    while [[ -L "$actual_script" ]]; do
        local target
        target="$(readlink "$actual_script")"
        if [[ "$target" == /* ]]; then
            actual_script="$target"
        else
            actual_script="$(cd "$(dirname "$actual_script")" && pwd)/$(basename "$target")"
        fi
    done

    # Replace the actual script file
    if mv "$tmpfile" "$actual_script"; then
        chmod +x "$actual_script"
        echo "Successfully upgraded to latest version!"
        echo "The Docker image will be rebuilt automatically on next run if needed."
    else
        echo "Error: Failed to replace script. You may need to run with sudo." >&2
        rm -f "$tmpfile"
        exit 1
    fi
}

# Subcommand dispatch
case "${1:-}" in
    new)
        shift
        if [[ $# -lt 1 ]]; then
            echo "Error: Session name is required." >&2
            echo "Usage: realm new <name> [project_dir] [claude args...]" >&2
            exit 1
        fi
        cmd_new "$@"
        ;;
    resume)
        shift
        if [[ $# -lt 1 ]]; then
            echo "Error: Session name is required." >&2
            echo "Usage: realm resume <name> [claude args...]" >&2
            exit 1
        fi
        cmd_resume "$@"
        ;;
    list)
        cmd_list
        ;;
    push)
        shift
        if [[ $# -lt 1 ]]; then
            echo "Error: Session name is required." >&2
            echo "Usage: realm push <name>" >&2
            exit 1
        fi
        cmd_push "$1"
        ;;
    remove)
        shift
        if [[ $# -lt 1 ]]; then
            echo "Error: Session name is required." >&2
            echo "Usage: realm remove <name>" >&2
            exit 1
        fi
        cmd_remove "$1"
        ;;
    upgrade)
        cmd_upgrade
        ;;
    *)
        usage
        ;;
esac
