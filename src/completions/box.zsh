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
    local curcontext="$curcontext" state line
    typeset -A opt_args

    _arguments -C \
        '1: :->subcmd' \
        '*:: :->args'

    case $state in
        subcmd)
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
            ;;
        args)
            case $words[1] in
                workspace|ws)
                    if (( CURRENT == 2 )); then
                        local -a ws_subcmds
                        ws_subcmds=(
                            'add:Create a new workspace'
                            'list:List workspaces'
                            'remove:Remove a workspace'
                            'switch:Switch into a workspace'
                        )
                        _describe 'workspace subcommand' ws_subcmds
                    else
                        case $words[2] in
                            add)
                                _arguments \
                                    '*--repo=[Select specific source]:source:__box_repos' \
                                    '--preset=[Use a preset]:preset:__box_presets' \
                                    '--strategy=[Workspace strategy]:strategy:(clone worktree)' \
                                    '--no-fetch[Skip git fetch before creating the workspace]'
                                ;;
                            list|ls)
                                _arguments \
                                    '(-q --quiet)'{-q,--quiet}'[Only print workspace names]'
                                ;;
                            remove|rm)
                                if [[ $words[CURRENT] == -* ]]; then
                                    _arguments '(-a --all)'{-a,--all}'[Remove every workspace]'
                                elif (( CURRENT == 3 )); then
                                    __box_sessions
                                fi
                                ;;
                            switch|sw)
                                if (( CURRENT == 3 )); then
                                    __box_sessions
                                fi
                                ;;
                        esac
                    fi
                    ;;
                repo)
                    if (( CURRENT == 2 )); then
                        local -a repo_subcmds
                        repo_subcmds=(
                            'add:Add repo(s) to a workspace or preset'
                            'remove:Remove repo(s) from a workspace or preset'
                            'list:List repos in a workspace or preset'
                        )
                        _describe 'repo subcommand' repo_subcmds
                    else
                        case $words[2] in
                            add|remove|rm)
                                _arguments \
                                    '--workspace=[Target workspace]:workspace:__box_sessions' \
                                    '--preset=[Target preset]:preset:__box_presets' \
                                    '*:source:__box_repos'
                                ;;
                            list|ls)
                                _arguments \
                                    '--workspace=[Target workspace]:workspace:__box_sessions' \
                                    '--preset=[Target preset]:preset:__box_presets'
                                ;;
                        esac
                    fi
                    ;;
                source)
                    if (( CURRENT == 2 )); then
                        local -a source_subcmds
                        source_subcmds=(
                            'add:Register a git repo as a source'
                            'remove:Unregister a source'
                            'list:List registered sources'
                        )
                        _describe 'source subcommand' source_subcmds
                    elif (( CURRENT == 3 )); then
                        case $words[2] in
                            remove|rm)
                                __box_repos
                                ;;
                            add)
                                _files -/
                                ;;
                        esac
                    fi
                    ;;
                preset)
                    if (( CURRENT == 2 )); then
                        local -a preset_subcmds
                        preset_subcmds=(
                            'add:Create or update a preset'
                            'remove:Remove a preset'
                            'list:List presets'
                        )
                        _describe 'preset subcommand' preset_subcmds
                    elif (( CURRENT == 3 )); then
                        case $words[2] in
                            remove|rm)
                                __box_presets
                                ;;
                        esac
                    elif [[ $words[2] == "add" ]]; then
                        _arguments \
                            '*--repo=[Select specific source]:source:__box_repos'
                    fi
                    ;;
                rebase)
                    _arguments \
                        '--workspace=[Workspace name]:workspace:__box_sessions' \
                        '--repo=[Repo within the workspace]:repo:__box_repos' \
                        '1:branch (e.g. main):'
                    ;;
                config)
                    if (( CURRENT == 2 )); then
                        local -a shells
                        shells=('zsh:Zsh completion script' 'bash:Bash completion script')
                        _describe 'shell' shells
                    fi
                    ;;
            esac
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
