__box_sessions_list() {
    local __box_root="${BOX_ROOT:-$HOME/.box}"
    local out=""
    if [[ -d "$__box_root/sessions" ]]; then
        for sess in "$__box_root/sessions"/*/; do
            ([[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]) && out+=" $(basename "$sess")"
        done
    fi
    echo "$out"
}

__box_repos_list() {
    local __box_root="${BOX_ROOT:-$HOME/.box}"
    local out=""
    if [[ -d "$__box_root/repos" ]]; then
        for bare in "$__box_root/repos"/*.git; do
            [[ -d "$bare" ]] || continue
            out+=" $(basename "$bare" .git)"
        done
    fi
    echo "$out"
}

__box_presets_list() {
    local __box_root="${BOX_ROOT:-$HOME/.box}"
    local out=""
    if [[ -d "$__box_root/presets" ]]; then
        for f in "$__box_root/presets"/*; do
            [[ -f "$f" ]] || continue
            out+=" $(basename "$f")"
        done
    fi
    echo "$out"
}

_box() {
    local cur prev words cword
    _init_completion || return

    local subcommands="workspace repo source preset rebase upgrade config"

    if [[ $cword -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$subcommands" -- "$cur"))
        return
    fi

    local sub="${words[1]}"
    [[ -z "$sub" ]] && return

    case "$sub" in
        workspace|ws)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add list remove switch" -- "$cur"))
                return
            fi
            case "${words[2]}" in
                add)
                    case "$prev" in
                        --strategy)
                            COMPREPLY=($(compgen -W "clone worktree" -- "$cur")); return ;;
                        --repo)
                            COMPREPLY=($(compgen -W "$(__box_repos_list)" -- "$cur")); return ;;
                        --preset)
                            COMPREPLY=($(compgen -W "$(__box_presets_list)" -- "$cur")); return ;;
                    esac
                    [[ "$cur" == -* ]] && COMPREPLY=($(compgen -W "--repo --preset --strategy --no-fetch" -- "$cur"))
                    ;;
                list|ls)
                    [[ "$cur" == -* ]] && COMPREPLY=($(compgen -W "--quiet -q" -- "$cur"))
                    ;;
                remove|rm)
                    if [[ "$cur" == -* ]]; then
                        COMPREPLY=($(compgen -W "--all -a" -- "$cur"))
                    elif [[ $cword -eq 3 ]]; then
                        COMPREPLY=($(compgen -W "$(__box_sessions_list)" -- "$cur"))
                    fi
                    ;;
                switch|sw)
                    if [[ $cword -eq 3 ]]; then
                        COMPREPLY=($(compgen -W "$(__box_sessions_list)" -- "$cur"))
                    fi
                    ;;
            esac
            ;;
        repo)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add remove list" -- "$cur"))
                return
            fi
            case "$prev" in
                --workspace)
                    COMPREPLY=($(compgen -W "$(__box_sessions_list)" -- "$cur")); return ;;
                --preset)
                    COMPREPLY=($(compgen -W "$(__box_presets_list)" -- "$cur")); return ;;
            esac
            case "${words[2]}" in
                add|remove|rm)
                    if [[ "$cur" == -* ]]; then
                        COMPREPLY=($(compgen -W "--workspace --preset" -- "$cur"))
                    else
                        COMPREPLY=($(compgen -W "$(__box_repos_list)" -- "$cur"))
                    fi
                    ;;
                list|ls)
                    [[ "$cur" == -* ]] && COMPREPLY=($(compgen -W "--workspace --preset" -- "$cur"))
                    ;;
            esac
            ;;
        source)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add remove list" -- "$cur"))
            elif [[ $cword -eq 3 ]]; then
                case "${words[2]}" in
                    remove|rm)
                        COMPREPLY=($(compgen -W "$(__box_repos_list)" -- "$cur"))
                        ;;
                    add)
                        COMPREPLY=($(compgen -d -- "$cur"))
                        ;;
                esac
            fi
            ;;
        preset)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add update remove list" -- "$cur"))
                return
            fi
            if [[ "$prev" == "--repo" ]]; then
                COMPREPLY=($(compgen -W "$(__box_repos_list)" -- "$cur")); return
            fi
            case "${words[2]}" in
                remove|rm)
                    [[ $cword -eq 3 ]] && COMPREPLY=($(compgen -W "$(__box_presets_list)" -- "$cur"))
                    ;;
                update)
                    if [[ $cword -eq 3 ]]; then
                        COMPREPLY=($(compgen -W "$(__box_presets_list)" -- "$cur"))
                    elif [[ "$cur" == -* ]]; then
                        COMPREPLY=($(compgen -W "--repo" -- "$cur"))
                    fi
                    ;;
                add)
                    [[ "$cur" == -* ]] && COMPREPLY=($(compgen -W "--repo" -- "$cur"))
                    ;;
            esac
            ;;
        rebase)
            case "$prev" in
                --workspace)
                    COMPREPLY=($(compgen -W "$(__box_sessions_list)" -- "$cur")); return ;;
                --repo)
                    COMPREPLY=($(compgen -W "$(__box_repos_list)" -- "$cur")); return ;;
            esac
            [[ "$cur" == -* ]] && COMPREPLY=($(compgen -W "--workspace --repo" -- "$cur"))
            ;;
        config)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "zsh bash" -- "$cur"))
            fi
            ;;
    esac
}
complete -F _box box

box() {
    local __box_cd_file __box_post_switch_file
    __box_cd_file=$(mktemp "/tmp/.box-cd.XXXXXX")
    __box_post_switch_file=$(mktemp "/tmp/.box-post-switch.XXXXXX")
    BOX_CD_FILE="$__box_cd_file" BOX_POST_SWITCH_FILE="$__box_post_switch_file" command box "$@"
    local __box_exit=$?
    if [[ -s "$__box_cd_file" ]]; then
        local __box_dir
        __box_dir=$(<"$__box_cd_file")
        cd "$__box_dir"
    fi
    if [[ -s "$__box_post_switch_file" ]] && [[ -n "$BOX_POST_SWITCH_HOOK" ]]; then
        local __box_name
        __box_name=$(<"$__box_post_switch_file")
        BOX_SESSION_NAME="$__box_name" eval "$BOX_POST_SWITCH_HOOK"
    fi
    rm -f "$__box_cd_file" "$__box_post_switch_file"
    return $__box_exit
}
