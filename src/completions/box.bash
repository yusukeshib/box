_box() {
    local cur prev words cword
    _init_completion || return

    local subcommands="new edit remove rm list switch sw cd rebase repo preset upgrade config"
    local session_cmds="edit remove rm switch sw cd"
    local __box_root="${BOX_ROOT:-$HOME/.box}"

    if [[ $cword -eq 1 ]]; then
        COMPREPLY=($(compgen -W "$subcommands" -- "$cur"))
        return
    fi

    local subcmd="${words[1]}"
    [[ -z "$subcmd" ]] && return

    case "$subcmd" in
        new)
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--repo --preset --strategy --no-fetch --verbose -v" -- "$cur"))
                    ;;
            esac
            if [[ "$prev" == "--strategy" ]]; then
                COMPREPLY=($(compgen -W "clone worktree" -- "$cur"))
            fi
            ;;
        list|ls)
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--project -p --quiet -q" -- "$cur"))
                    ;;
            esac
            ;;
        edit)
            if [[ "$prev" == "--add" || "$prev" == "--remove" ]]; then
                local repos=""
                if [[ -d "$__box_root/repos" ]]; then
                    for bare in "$__box_root/repos"/*.git; do
                        [[ -d "$bare" ]] || continue
                        local name=$(basename "$bare" .git)
                        [[ -n "$name" ]] && repos+=" $name"
                    done
                fi
                COMPREPLY=($(compgen -W "$repos" -- "$cur"))
                return
            fi
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--add --remove --verbose -v" -- "$cur"))
                    ;;
                *)
                    if [[ $cword -eq 2 ]]; then
                        local sessions=""
                        if [[ -d "$__box_root/sessions" ]]; then
                            for sess in "$__box_root/sessions"/*/; do
                                ([[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]) && sessions+=" $(basename "$sess")"
                            done
                        fi
                        COMPREPLY=($(compgen -W "$sessions" -- "$cur"))
                    fi
                    ;;
            esac
            ;;
        remove|rm)
            case "$cur" in
                -*)
                    COMPREPLY=($(compgen -W "--all -a --verbose -v" -- "$cur"))
                    ;;
                *)
                    if [[ $cword -eq 2 ]]; then
                        local sessions=""
                        if [[ -d "$__box_root/sessions" ]]; then
                            for sess in "$__box_root/sessions"/*/; do
                                ([[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]) && sessions+=" $(basename "$sess")"
                            done
                        fi
                        COMPREPLY=($(compgen -W "$sessions" -- "$cur"))
                    fi
                    ;;
            esac
            ;;
        switch|sw|cd)
            if [[ $cword -eq 2 ]]; then
                local sessions=""
                if [[ -d "$__box_root/sessions" ]]; then
                    for sess in "$__box_root/sessions"/*/; do
                        ([[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]) && sessions+=" $(basename "$sess")"
                    done
                fi
                COMPREPLY=($(compgen -W "$sessions" -- "$cur"))
            fi
            ;;
        repo)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add remove rm list ls" -- "$cur"))
            elif [[ $cword -eq 3 ]]; then
                case "${words[2]}" in
                    remove|rm)
                        local repos=""
                        if [[ -d "$__box_root/repos" ]]; then
                            for bare in "$__box_root/repos"/*.git; do
                                [[ -d "$bare" ]] || continue
                                local name=$(basename "$bare" .git)
                                [[ -n "$name" ]] && repos+=" $name"
                            done
                        fi
                        COMPREPLY=($(compgen -W "$repos" -- "$cur"))
                        ;;
                    add)
                        COMPREPLY=($(compgen -d -- "$cur"))
                        ;;
                esac
            fi
            ;;
        preset)
            if [[ $cword -eq 2 ]]; then
                COMPREPLY=($(compgen -W "add edit remove rm list ls" -- "$cur"))
            elif [[ $cword -eq 3 ]]; then
                case "${words[2]}" in
                    edit|remove|rm)
                        local presets=""
                        if [[ -d "$__box_root/presets" ]]; then
                            for f in "$__box_root/presets"/*; do
                                [[ -f "$f" ]] || continue
                                presets+=" $(basename "$f")"
                            done
                        fi
                        COMPREPLY=($(compgen -W "$presets" -- "$cur"))
                        ;;
                esac
            fi
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
