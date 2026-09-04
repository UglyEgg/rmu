# Fish completion for rmuctl.

complete -c rmuctl -f
complete -c rmuctl -n 'not __fish_seen_subcommand_from list info restore purge empty doctor completions' -a list -d 'List Trash entries'
complete -c rmuctl -n 'not __fish_seen_subcommand_from list info restore purge empty doctor completions' -a info -d 'Show entry details'
complete -c rmuctl -n 'not __fish_seen_subcommand_from list info restore purge empty doctor completions' -a restore -d 'Restore entries'
complete -c rmuctl -n 'not __fish_seen_subcommand_from list info restore purge empty doctor completions' -a purge -d 'Permanently remove selected entries'
complete -c rmuctl -n 'not __fish_seen_subcommand_from list info restore purge empty doctor completions' -a empty -d 'Permanently empty matching entries'
complete -c rmuctl -n 'not __fish_seen_subcommand_from list info restore purge empty doctor completions' -a doctor -d 'Diagnose Trash state'
complete -c rmuctl -n 'not __fish_seen_subcommand_from list info restore purge empty doctor completions' -a completions -d 'Print a shell completion definition'

complete -c rmuctl -n '__fish_seen_subcommand_from completions; and not __fish_seen_subcommand_from bash zsh fish' -a 'bash zsh fish' -d 'Completion shell'
complete -c rmuctl -n '__fish_seen_subcommand_from completions; and __fish_seen_subcommand_from bash zsh fish; and not __fish_seen_subcommand_from rmu rmuctl' -a 'rmu rmuctl' -d 'Command to complete'

complete -c rmuctl -n 'not __fish_seen_subcommand_from completions' -l json -d 'Emit JSON or JSON Lines'
complete -c rmuctl -l null -n '__fish_seen_subcommand_from list' -d 'Terminate list records with NUL'
complete -c rmuctl -n 'not __fish_seen_subcommand_from completions' -s q -l quiet -d 'Suppress successful action output'
complete -c rmuctl -n 'not __fish_seen_subcommand_from completions' -l no-color -d 'Disable color'
complete -c rmuctl -l help -d 'Display help and exit'
complete -c rmuctl -l version -d 'Display version and exit'

complete -c rmuctl -n '__fish_seen_subcommand_from list empty' -l filesystem -x -a '(__fish_complete_directories)' -d 'Select a filesystem'
complete -c rmuctl -n '__fish_seen_subcommand_from list empty' -l older-than -x -a '30d 12h 90m 10s' -d 'Select older entries'
complete -c rmuctl -n '__fish_seen_subcommand_from list' -l newer-than -x -a '30d 12h 90m 10s' -d 'Select newer entries'
complete -c rmuctl -n '__fish_seen_subcommand_from list' -l sort -x -a 'name date size path' -d 'Select sort field'
complete -c rmuctl -n '__fish_seen_subcommand_from list' -l reverse -d 'Reverse the ordering'

complete -c rmuctl -n '__fish_seen_subcommand_from restore' -l to -x -a '(__fish_complete_directories)' -d 'Restore below a directory'
complete -c rmuctl -n '__fish_seen_subcommand_from restore' -l conflict -x -a 'fail rename' -d 'Select conflict policy'
complete -c rmuctl -n '__fish_seen_subcommand_from restore' -l parents -d 'Create missing destination parents'

complete -c rmuctl -n '__fish_seen_subcommand_from purge empty' -l yes -d 'Confirm permanent deletion'
complete -c rmuctl -n '__fish_seen_subcommand_from purge empty' -l progress -d 'Show processed entries; values use --progress=WHEN'
complete -c rmuctl -f -n '__fish_seen_subcommand_from purge empty; and string match -q -- "--progress=*" (commandline -ct)' -a '--progress=auto --progress=always --progress=never'

complete -c rmuctl -n '__fish_seen_subcommand_from doctor' -l repair-cache -d 'Rebuild directory size caches'
complete -c rmuctl -n '__fish_seen_subcommand_from doctor' -l recover-orphans -x -a '(__fish_complete_directories)' -d 'Move recoverable orphans'
complete -c rmuctl -n '__fish_seen_subcommand_from restore purge empty doctor' -l dry-run -d 'Report without changing anything'
