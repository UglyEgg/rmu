# Bash completion for RMU.

_rmu_complete_files()
{
    if declare -F _filedir >/dev/null; then
        _filedir
    else
        mapfile -t COMPREPLY < <(compgen -f -- "$cur")
    fi
}

_rmu()
{
    local cur prev
    COMPREPLY=()
    cur=${COMP_WORDS[COMP_CWORD]}
    prev=${COMP_WORDS[COMP_CWORD-1]}

    case $cur in
        --interactive=*)
            COMPREPLY=( $(compgen -W 'never once always' -- "${cur#*=}") )
            COMPREPLY=( "${COMPREPLY[@]/#/--interactive=}" )
            return
            ;;
        --preserve-root=*)
            COMPREPLY=( $(compgen -W 'all' -- "${cur#*=}") )
            COMPREPLY=( "${COMPREPLY[@]/#/--preserve-root=}" )
            return
            ;;
        --rmu-progress=*)
            COMPREPLY=( $(compgen -W 'auto always never' -- "${cur#*=}") )
            COMPREPLY=( "${COMPREPLY[@]/#/--rmu-progress=}" )
            return
            ;;
        -*)
            COMPREPLY=( $(compgen -W '-f --force -i -I --interactive -d --dir --one-file-system --no-preserve-root --preserve-root -r -R --recursive -v --verbose --trash --delegate --rmu-progress --help --version' -- "$cur") )
            return
            ;;
    esac

    _rmu_complete_files
}

complete -F _rmu rmu
