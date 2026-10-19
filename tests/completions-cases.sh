#!/usr/bin/env bash
# Shared cases, executed independently by Bash and Zsh via completions.sh.
# Stub only framework entry points; use the real workspace discovery helpers.
if [ -n "${ZSH_VERSION:-}" ]; then
    compdef() { :; }
    _describe() {
        local candidate
        for candidate in "${(@P)2}"; do
            candidate=${candidate%%:*}
            [[ $candidate == "${words[CURRENT]}"* ]] && results+=("$candidate")
        done
        return 0
    }
    source "$BOX_COMPLETION_ROOT/src/completions/box.zsh"
else
    _init_completion() {
        words=("${COMP_WORDS[@]}")
        cword=$COMP_CWORD
        cur=${words[cword]}
        prev=${words[cword-1]}
    }
    source "$BOX_COMPLETION_ROOT/src/completions/box.bash"
fi

check() {
    local expected=$1 actual
    shift
    results=()
    COMPREPLY=()
    if [ -n "${ZSH_VERSION:-}" ]; then
        words=(box "$@")
        CURRENT=${#words[@]}
    else
        COMP_WORDS=(box "$@")
        COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
    fi
    _box
    if [ -z "${ZSH_VERSION:-}" ]; then
        results=("${COMPREPLY[@]}")
    fi
    actual=$(printf '%s\n' "${results[@]}" | LC_ALL=C sort)
    if [ "$actual" != "$expected" ]; then
        printf '%s: completion failed for [%s]\nexpected: [%s]\nactual: [%s]\n' \
            "${ZSH_VERSION:-bash}" "$*" "$expected" "$actual" >&2
        exit 1
    fi
}

names=$(printf 'alpha\nbeta')
verbose=$(printf '%s\n' '--verbose' '-v' | LC_ALL=C sort)
all_options=$(printf '%s\n' '--all' '--verbose' '-a' '-v' | LC_ALL=C sort)
for workspace in workspace ws; do
    for remove in remove rm; do
        check "$names" "$workspace" "$remove" ''
        check beta "$workspace" "$remove" alpha b
        check "$names" "$workspace" "$remove" alpha beta ''
        check "$names" "$workspace" "$remove" --verbose alpha ''
        check "$all_options" "$workspace" "$remove" -
        check "$verbose" "$workspace" "$remove" alpha -
        for all in --all -a; do
            check '' "$workspace" "$remove" "$all" ''
            check '' "$workspace" "$remove" "$all" b
            check "$verbose" "$workspace" "$remove" "$all" -
            check '' "$workspace" "$remove" --verbose "$all" ''
        done
        check "$names" "$workspace" "$remove" -- --all ''
    done
done
printf '%s completion tests passed\n' "${ZSH_VERSION:-bash}"
