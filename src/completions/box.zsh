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
        _describe 'session' sessions
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
        _describe 'repo' repos
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
                'new:Create a new session'
                'edit:Edit repos in an existing session'
                'remove:Remove a session'
                'rm:Remove a session'
                'list:List sessions'
                'switch:Switch to a session'
                'sw:Switch to a session'
                'cd:Switch to a session'
                'rebase:Fetch origin and rebase the current branch'
                'repo:Manage registered repos'
                'preset:Manage session presets'
                'upgrade:Self-update to the latest version'
                'config:Output shell configuration'
            )
            _describe 'subcommand' subcmds
            ;;
        args)
            case $words[1] in
                new)
                    _arguments \
                        '*--repo=[Select specific repo]:repo:__box_repos' \
                        '--preset=[Use a preset]:preset:__box_presets' \
                        '--strategy=[Workspace strategy]:strategy:(clone worktree)' \
                        '--no-fetch[Skip git fetch before creating the workspace]' \
                        '(-v --verbose)'{-v,--verbose}'[Show detailed output]' \
                        '1:session name:' \
                        '*:command:'
                    ;;
                list|ls)
                    _arguments \
                        '--project[Show only sessions for the current project]' \
                        '-p[Show only sessions for the current project]' \
                        '--quiet[Only print session names]' \
                        '-q[Only print session names]'
                    ;;
                remove|rm)
                    _arguments \
                        '(-v --verbose)'{-v,--verbose}'[Show detailed output]' \
                        '(-a --all)'{-a,--all}'[Remove every session]' \
                        '1:session name:__box_sessions'
                    ;;
                edit)
                    _arguments \
                        '(-v --verbose)'{-v,--verbose}'[Show detailed output]' \
                        '*--add=[Add a repo to the session]:repo:__box_repos' \
                        '*--remove=[Remove a repo from the session]:repo:__box_repos' \
                        '1:session name:__box_sessions'
                    ;;
                switch|sw|cd)
                    if (( CURRENT == 2 )); then
                        __box_sessions
                    fi
                    ;;
                rebase)
                    if (( CURRENT == 2 )); then
                        _message 'branch (e.g. main)'
                    fi
                    ;;
                repo)
                    if (( CURRENT == 2 )); then
                        local -a repo_subcmds
                        repo_subcmds=('add:Register a git repo' 'remove:Unregister a repo' 'rm:Unregister a repo' 'list:List registered repos' 'ls:List registered repos')
                        _describe 'repo subcommand' repo_subcmds
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
                        preset_subcmds=('add:Create or update a preset' 'edit:Edit repos in an existing preset' 'remove:Remove a preset' 'rm:Remove a preset' 'list:List presets' 'ls:List presets')
                        _describe 'preset subcommand' preset_subcmds
                    elif (( CURRENT == 3 )); then
                        case $words[2] in
                            edit|remove|rm)
                                __box_presets
                                ;;
                        esac
                    elif [[ $words[2] == "add" ]]; then
                        _arguments \
                            '*--repo=[Select specific repo]:repo:__box_repos'
                    fi
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
    local __box_cd_file __box_rename_file
    __box_cd_file=$(mktemp "/tmp/.box-cd.XXXXXX")
    __box_rename_file=$(mktemp "/tmp/.box-rename.XXXXXX")
    BOX_CD_FILE="$__box_cd_file" BOX_RENAME_FILE="$__box_rename_file" command box "$@"
    local __box_exit=$?
    if [[ -s "$__box_cd_file" ]]; then
        local __box_dir
        __box_dir=$(<"$__box_cd_file")
        cd "$__box_dir"
    fi
    if [[ -s "$__box_rename_file" ]]; then
        local __box_name
        __box_name=$(<"$__box_rename_file")
        if [[ -n "$ZELLIJ" ]]; then
            command zellij action rename-tab "$__box_name" 2>/dev/null
        elif [[ -n "$TMUX" ]]; then
            command tmux rename-window -t "$TMUX_PANE" "$__box_name" 2>/dev/null
        fi
    fi
    rm -f "$__box_cd_file" "$__box_rename_file"
    return $__box_exit
}
