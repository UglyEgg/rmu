# Security policy

## Supported versions

Until RMU reaches 1.0, only the newest published release receives security and
data-safety fixes.

## Reporting a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/UglyEgg/rmu/security/advisories/new).
The repository owner must enable that facility before making the repository
public. If the link is unexpectedly unavailable, do not disclose security
details in public: open a minimal issue that only asks the owner to restore a
private reporting channel.

In the private report, include the RMU version, Linux distribution, filesystem
and mount layout, exact argument vector, relevant environment variables, and a
minimal reproduction that does not contain sensitive filenames.

Particularly sensitive reports include unintended permanent deletion,
overwrites, symlink or mount-boundary escapes, use of an unsafe Trash root,
incorrect confirmation behavior, terminal-control injection, and stale entry
IDs acting on a different Trash incarnation.

## Security boundaries

- RMU supports local Linux filesystems and the FreeDesktop Trash format. It
  does not promise remote-GIO behavior.
- It refuses permanent fallback when no safe Trash can be selected.
- `rmu --delegate` and `RMU_MODE=delegate` deliberately leave the Trash
  safety model and execute GNU rm. RMU checks the selected executable's GNU
  identity and refuses a path resolving back to itself, but executable search
  still trusts `PATH` and an explicit `RMU_GNU_RM` in the same way other
  command launch does. Do not place untrusted writable directories on `PATH`.
- `rmu -d` is intentionally permanent for an empty directory, matching GNU
  `rm -d`; `rmuctl purge` and `empty` are also permanent and require a terminal
  confirmation or explicit `--yes`.
- Filesystem contents and mount topology may change concurrently. RMU pins and
  revalidates identities, but the FreeDesktop specification defines no lock
  shared with desktop Trash cleaners. Do not empty Trash concurrently with an
  active `rmu` operation.
- Full power-loss atomicity is not claimed because containing directories are
  not fsynced and there is no recovery journal.

The detailed mechanism and known semantic boundaries are documented in
[architecture.md](architecture.md) and [compatibility.md](compatibility.md).
