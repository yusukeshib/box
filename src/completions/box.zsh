__box_sessions() {
    local -a sessions
    local __box_root="${BOX_ROOT:-$HOME/.box}"
    if [[ -d "$__box_root/sessions" ]]; then
        for sess in "$__box_root/sessions"/*(N/); do
            if [[ -f "$sess/project_dir" ]] || [[ -f "$sess/repos" ]]; then
                local sess_name=${sess:t}
                local desc=""
                if [[ -f "$sess/project_dir" ]]; then
                    desc=$(< "$sess/project_dir")
                    desc=${desc/#$HOME/\~}
                fi
                if [[ -n "$desc" ]]; then
                    sessions+=("$sess_name:$desc")
                else
                    sessions+=("$sess_name")
                fi
            fi
        done
    fi
    if (( ${#sessions} )); then
        _describe 'workspace' sessions
    fi
}

__box_repos() {
    local -a repos
    local __box_root="${BOX_ROOT:-$HOME/.box}"
    if [[ -d "$__box_root/repos" ]]; then
        for bare in "$__box_root/repos"/*.git(N/); do
            local name=${bare:t}
            name=${name%.git}
            [[ -n "$name" ]] && repos+=("$name")
        done
    fi
    if (( ${#repos} )); then
        _describe 'source' repos
    fi
}

__box_presets() {
    local -a presets
    local __box_root="${BOX_ROOT:-$HOME/.box}"
    if [[ -d "$__box_root/presets" ]]; then
        for preset in "$__box_root/presets"/*(N.); do
            local name=${preset:t}
            [[ -n "$name" ]] && presets+=("$name")
        done
    fi
    if (( ${#presets} )); then
        _describe 'preset' presets
    fi
}

_box() {
    local curcontext="$curcontext"
    local prev="${words[CURRENT-1]}"

    # In zsh, words[1] is the command itself. Keep all candidates canonical;
    # aliases such as ws/rm/ls/sw are accepted below but never suggested.
    if (( CURRENT == 2 )); then
        if [[ $words[CURRENT] == -* ]]; then
            local -a global_options
            global_options=(
                '--verbose:Show detailed output'
                '-v:Show detailed output'
            )
            _describe 'option' global_options
        else
            local -a subcmds
            subcmds=(
                'workspace:Manage workspaces (create, list, switch, remove)'
                'repo:Manage repos within a workspace or preset'
                'source:Manage registered sources (upstream git repos)'
                'preset:Manage presets'
                'rebase:Fetch origin and rebase a workspace repo'
                'upgrade:Self-update to the latest version'
                'config:Output shell configuration'
            )
            _describe 'subcommand' subcmds
        fi
        return
    fi

    case $words[2] in
        workspace|ws)
            if (( CURRENT == 3 )); then
                local -a ws_subcmds
                ws_subcmds=(
                    'add:Create a new workspace'
                    'list:List workspaces'
                    'remove:Remove one workspace (or --all)'
                    'prune:Remove workspaces older than a given age'
                    'switch:Switch into a workspace'
                )
                _describe 'workspace subcommand' ws_subcmds
                return
            fi
            case $words[3] in
                add)
                    case $prev in
                        --repo) __box_repos; return ;;
                        --preset) __box_presets; return ;;
                        --strategy) compadd clone worktree; return ;;
                    esac
                    if [[ $words[CURRENT] == -* || ( CURRENT -ge 5 && -z $words[CURRENT] ) ]]; then
                        local -a add_options
                        add_options=(
                            '--repo:Select specific source'
                            '--preset:Use a preset'
                            '--strategy:Workspace strategy'
                            '--no-fetch:Skip git fetch before creating the workspace'
                        )
                        _describe 'option' add_options
                    fi
                    ;;
                list|ls)
                    if [[ $words[CURRENT] == -* ]]; then
                        local -a list_options
                        list_options=(
                            '--quiet:Only print workspace names'
                            '-q:Only print workspace names'
                        )
                        _describe 'option' list_options
                    fi
                    ;;
                prune)
                    if [[ $prev == --older-than ]]; then
                        compadd 3d 7d 30d
                        return
                    elif [[ $words[CURRENT] == -* ]]; then
                        local -a prune_options
                        prune_options=(
                            '--older-than:Prune workspaces at least this old (default: 3d)'
                        )
                        _describe 'option' prune_options
                    fi
                    ;;
                remove|rm)
                    if [[ $words[CURRENT] == -* ]]; then
                        local -a remove_options
                        remove_options=(
                            '--all:Remove every workspace'
                            '-a:Remove every workspace'
                        )
                        _describe 'option' remove_options
                    elif (( CURRENT == 4 )); then
                        __box_sessions
                    fi
                    ;;
                switch|sw)
                    (( CURRENT == 4 )) && __box_sessions
                    ;;
            esac
            ;;
        repo)
            if (( CURRENT == 3 )); then
                local -a repo_subcmds
                repo_subcmds=(
                    'add:Add repo(s) to a workspace or preset'
                    'remove:Remove repo(s) from a workspace or preset'
                    'list:List repos in a workspace or preset'
                )
                _describe 'repo subcommand' repo_subcmds
                return
            fi
            case $prev in
                --workspace) __box_sessions; return ;;
                --preset) __box_presets; return ;;
            esac
            case $words[3] in
                add|remove|rm)
                    if [[ $words[CURRENT] == -* ]]; then
                        local -a repo_options
                        repo_options=(
                            '--workspace:Target workspace'
                            '--preset:Target preset'
                        )
                        _describe 'option' repo_options
                    else
                        __box_repos
                    fi
                    ;;
                list|ls)
                    if [[ $words[CURRENT] == -* ]]; then
                        local -a repo_list_options
                        repo_list_options=(
                            '--workspace:Target workspace'
                            '--preset:Target preset'
                        )
                        _describe 'option' repo_list_options
                    fi
                    ;;
            esac
            ;;
        source)
            if (( CURRENT == 3 )); then
                local -a source_subcmds
                source_subcmds=(
                    'add:Register a git repo as a source'
                    'remove:Unregister a source'
                    'list:List registered sources'
                )
                _describe 'source subcommand' source_subcmds
            elif (( CURRENT == 4 )); then
                case $words[3] in
                    remove|rm) __box_repos ;;
                    add) _files -/ ;;
                esac
            fi
            ;;
        preset)
            if (( CURRENT == 3 )); then
                local -a preset_subcmds
                preset_subcmds=(
                    'add:Create a new preset'
                    'update:Replace an existing preset'"'"'s repos'
                    'remove:Remove a preset'
                    'list:List presets'
                )
                _describe 'preset subcommand' preset_subcmds
                return
            fi
            if [[ $prev == --repo ]]; then
                __box_repos
                return
            fi
            case $words[3] in
                remove|rm)
                    (( CURRENT == 4 )) && __box_presets
                    ;;
                update)
                    if (( CURRENT == 4 )); then
                        __box_presets
                    elif [[ $words[CURRENT] == -* || ( CURRENT -ge 5 && -z $words[CURRENT] ) ]]; then
                        local -a preset_options
                        preset_options=('--repo:Select specific source')
                        _describe 'option' preset_options
                    fi
                    ;;
                add)
                    if [[ $words[CURRENT] == -* || ( CURRENT -ge 5 && -z $words[CURRENT] ) ]]; then
                        local -a preset_options
                        preset_options=('--repo:Select specific source')
                        _describe 'option' preset_options
                    fi
                    ;;
            esac
            ;;
        rebase)
            case $prev in
                --workspace) __box_sessions; return ;;
                --repo) __box_repos; return ;;
            esac
            if [[ $words[CURRENT] == -* ]]; then
                local -a rebase_options
                rebase_options=(
                    '--workspace:Workspace name'
                    '--repo:Repo within the workspace'
                )
                _describe 'option' rebase_options
            fi
            ;;
        config)
            if (( CURRENT == 3 )); then
                local -a shells
                shells=('zsh:Zsh completion script' 'bash:Bash completion script')
                _describe 'shell' shells
            fi
            ;;
    esac
}
compdef _box box

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
