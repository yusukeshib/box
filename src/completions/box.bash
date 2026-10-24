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

    local subcommands="workspace repo source preset upgrade config"

    if [[ $cword -eq 1 ]]; then
        if [[ "$cur" == -* ]]; then
            COMPREPLY=($(compgen -W "--verbose -v" -- "$cur"))
        else
            COMPREPLY=($(compgen -W "$subcommands" -- "$cur"))
        fi
        return
    fi

    local sub="${words[1]}"
    [[ -z "$sub" ]] && return

    case "$sub" in
        workspace|ws)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add list remove prune update switch" -- "$cur"))
                return
            fi
            case "${words[2]}" in
                add)
                    case "$prev" in
                        --strategy)
                            COMPREPLY=($(compgen -W "worktree clone layer" -- "$cur")); return ;;
                        --repo)
                            COMPREPLY=($(compgen -W "$(__box_repos_list)" -- "$cur")); return ;;
                        --preset)
                            COMPREPLY=($(compgen -W "$(__box_presets_list)" -- "$cur")); return ;;
                    esac
                    [[ "$cur" == -* || ( $cword -ge 4 && -z "$cur" ) ]] && COMPREPLY=($(compgen -W "--repo --preset --strategy --update --no-update" -- "$cur"))
                    ;;
                list|ls)
                    [[ "$cur" == -* ]] && COMPREPLY=($(compgen -W "--quiet -q" -- "$cur"))
                    ;;
                prune)
                    if [[ "$prev" == "--older-than" ]]; then
                        COMPREPLY=($(compgen -W "3d 7d 30d" -- "$cur"))
                    elif [[ "$cur" == -* ]]; then
                        COMPREPLY=($(compgen -W "--older-than" -- "$cur"))
                    fi
                    ;;
                remove|rm|update)
                    local has_names=0 has_all=0 end_options=0 i
                    for ((i = 3; i < cword; i++)); do
                        if (( end_options )); then
                            has_names=1
                        else
                            case "${words[i]}" in
                                --all|-a) has_all=1 ;;
                                --) end_options=1 ;;
                                -*) ;;
                                *) has_names=1 ;;
                            esac
                        fi
                    done
                    if [[ "$cur" == -* ]] && (( ! end_options )); then
                        local remove_options="--verbose -v"
                        (( has_names || has_all )) || remove_options+=" --all -a"
                        COMPREPLY=($(compgen -W "$remove_options" -- "$cur"))
                    elif (( ! has_all )); then
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
                    elif [[ "$cur" == -* || ( $cword -ge 4 && -z "$cur" ) ]]; then
                        COMPREPLY=($(compgen -W "--repo" -- "$cur"))
                    fi
                    ;;
                add)
                    [[ "$cur" == -* || ( $cword -ge 4 && -z "$cur" ) ]] && COMPREPLY=($(compgen -W "--repo" -- "$cur"))
                    ;;
            esac
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
    local __box_cd_file
    __box_cd_file=$(mktemp "/tmp/.box-cd.XXXXXX")
    BOX_CD_FILE="$__box_cd_file" command box "$@"
    local __box_exit=$?
    if [[ -s "$__box_cd_file" ]]; then
        local __box_dir
        __box_dir=$(<"$__box_cd_file")
        cd "$__box_dir"
    fi
    rm -f "$__box_cd_file"
    return $__box_exit
}
