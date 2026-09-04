# GNU rm compatibility contract

RMU's foundation is compatibility with GNU coreutils `rm` at the command-line
decision layer: parsing, option ordering, prompting mode, missing operands,
partial failure, and exit status. The test suite runs shared argument vectors
against a discovered GNU `rm` and `rmu` in separate disposable trees.

Trash mode is the default. `--trash` or `RMU_MODE=trash` can make that choice
explicit. When exact GNU behavior is required, `--delegate` or
`RMU_MODE=delegate` selects delegate mode. RMU then removes its exact mode
selectors and replaces itself with a verified GNU coreutils `rm`, preserving
GNU's own parsing, prompts, recursive traversal, output, signals, and exit
status. The command line overrides the environment, and the last mode selector
wins.

## Covered interface

`-f`, `-i`, `-I`, `--interactive`, `-d`, `--one-file-system`,
`--no-preserve-root`, `--preserve-root`, `-r`, `-R`, `--recursive`, `-v`,
`--help`, and `--version` are accepted, including grouped short options,
GNU-permitted long-option abbreviations, options after operands, and `--`.
Like GNU rm, RMU requires the exact spelling of `--no-preserve-root` rather
than accepting an abbreviation. Arbitrary non-UTF-8 Unix names are preserved.

`--trash`, `--delegate`, `--rmu-progress[=never|auto|always]`, `RMU_MODE`, and
`RMU_PROGRESS` are namespaced
extensions. With progress disabled, ordinary output remains GNU-shaped; Trash
diagnostic wording necessarily differs.

GNU's `POSIXLY_CORRECT` ordering is also honored: when the variable is present,
the first operand ends option parsing and all later words are operands. The
differential suite includes an option-looking filename after an ordinary file.

## Intentional semantic differences

- GNU `rm -r` walks and unlinks descendants. RMU validates and moves each
  top-level directory as one item, so descendant prompts and verbose lines do
  not exist.
- `--one-file-system` refuses a whole tree containing a nested mount. RMU
  cannot represent GNU `rm`'s partially removed tree as one Trash entry.
- `/` is never movable even after `--no-preserve-root`.
- A trailing-slash symlink is refused without following it.
- A bind-mounted alias of the home filesystem is refused when it cannot be
  renamed atomically into the home Trash.
- `-d` permanently removes an empty directory. Use `-r` to preserve even an
  empty directory and its metadata in Trash.
- Trash-mode diagnostics and prompts are English. Dynamically linked glibc
  builds use the locale's affirmative-response expression, while the static
  musl release accepts ASCII `y` or `yes`; delegate mode retains GNU rm's own
  localization and response handling.
- Trash mode records a non-interactive output failure, continues processing
  later operands so requested recoverable moves are not silently abandoned,
  and exits nonzero afterward. GNU rm may instead terminate on `SIGPIPE` and
  leave later operands untouched. Delegate mode retains GNU rm's signal and
  output behavior.

No recoverable operation falls back to permanent deletion. A failure to select
or publish to a safe FreeDesktop Trash is a failure.

## Oracle use

Set `RMU_GNU_RM=/path/to/rm` to force the delegate-mode and oracle executable.
Runtime delegation and tests reject a non-GNU implementation. Tests fail when
GNU `rm` is unavailable, preventing a missing comparison from looking like a
pass. Delegation also refuses to execute RMU itself, avoiding a recursive
`rm`-symlink configuration.

BSD `rm` remains a useful implementation reference, particularly for
descriptor-relative traversal and defensive deletion techniques, but it is
not the public parser oracle: BSD and GNU expose different documented options.
