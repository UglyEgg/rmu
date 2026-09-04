# Bash completion for rmuctl.

_rmuctl_complete_directories()
{
    if declare -F _filedir >/dev/null; then
        _filedir -d
    else
        mapfile -t COMPREPLY < <(compgen -d -- "$cur")
    fi
}

_rmuctl()
{
    local cur prev command word
    COMPREPLY=()
    cur=${COMP_WORDS[COMP_CWORD]}
    prev=${COMP_WORDS[COMP_CWORD-1]}

    for word in "${COMP_WORDS[@]:1}"; do
        case $word in
            list|info|restore|purge|empty|doctor|completions)
                command=$word
                break
                ;;
        esac
    done

    case $prev in
        --filesystem|--to|--recover-orphans)
            _rmuctl_complete_directories
            return
            ;;
        --sort)
            COMPREPLY=( $(compgen -W 'name date size path' -- "$cur") )
            return
            ;;
        --conflict)
            COMPREPLY=( $(compgen -W 'fail rename' -- "$cur") )
            return
            ;;
        --older-than|--newer-than)
            COMPREPLY=( $(compgen -W '30d 12h 90m 10s' -- "$cur") )
            return
            ;;
    esac

    case $cur in
        --sort=*)
            COMPREPLY=( $(compgen -W 'name date size path' -- "${cur#*=}") )
            COMPREPLY=( "${COMPREPLY[@]/#/--sort=}" )
            return
            ;;
        --conflict=*)
            COMPREPLY=( $(compgen -W 'fail rename' -- "${cur#*=}") )
            COMPREPLY=( "${COMPREPLY[@]/#/--conflict=}" )
            return
            ;;
        --progress=*)
            COMPREPLY=( $(compgen -W 'auto always never' -- "${cur#*=}") )
            COMPREPLY=( "${COMPREPLY[@]/#/--progress=}" )
            return
            ;;
    esac

    if [[ -z $command ]]; then
        COMPREPLY=( $(compgen -W 'list info restore purge empty doctor completions --json --null -q --quiet --no-color --help --version' -- "$cur") )
        return
    fi

    if [[ $command == completions ]]; then
        if [[ $cur == -* ]]; then
            COMPREPLY=( $(compgen -W '--help --version' -- "$cur") )
            return
        fi
        case $prev in
            completions)
                COMPREPLY=( $(compgen -W 'bash zsh fish' -- "$cur") )
                ;;
            bash|zsh|fish)
                COMPREPLY=( $(compgen -W 'rmu rmuctl' -- "$cur") )
                ;;
        esac
        return
    fi

    if [[ $cur == -* ]]; then
        case $command in
            list)
                COMPREPLY=( $(compgen -W '--filesystem --older-than --newer-than --sort --reverse --json --null -q --quiet --no-color --help --version' -- "$cur") )
                ;;
            info)
                COMPREPLY=( $(compgen -W '--json -q --quiet --no-color --help --version' -- "$cur") )
                ;;
            restore)
                COMPREPLY=( $(compgen -W '--to --conflict --parents --dry-run --json -q --quiet --no-color --help --version' -- "$cur") )
                ;;
            purge)
                COMPREPLY=( $(compgen -W '--dry-run --yes --progress --json -q --quiet --no-color --help --version' -- "$cur") )
                ;;
            empty)
                COMPREPLY=( $(compgen -W '--filesystem --older-than --dry-run --yes --progress --json -q --quiet --no-color --help --version' -- "$cur") )
                ;;
            doctor)
                COMPREPLY=( $(compgen -W '--repair-cache --recover-orphans --dry-run --json -q --quiet --no-color --help --version' -- "$cur") )
                ;;
        esac
    fi
}

complete -F _rmuctl rmuctl
