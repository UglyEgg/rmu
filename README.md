# RMU

<!-- markdownlint-disable-next-line MD013 -->
[![CI](https://github.com/UglyEgg/rmu/actions/workflows/ci.yml/badge.svg)](https://github.com/UglyEgg/rmu/actions/workflows/ci.yml) [![Latest release](https://img.shields.io/github/v/release/UglyEgg/rmu?display_name=tag&sort=semver)](https://github.com/UglyEgg/rmu/releases/latest) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE) [![MSRV: Rust 1.85](https://img.shields.io/badge/MSRV-Rust%201.85-orange.svg)](Cargo.toml) [![Platform: Linux](https://img.shields.io/badge/platform-Linux-lightgrey.svg)](docs/compatibility.md)

**Recoverable Move Utility — `rm` you can undo.**

`rmu` accepts GNU `rm` arguments but moves files into the FreeDesktop Trash by
default. Files appear in the same Trash used by KDE Plasma, GNOME, Xfce,
Cinnamon, and other conforming Linux desktops. `rmuctl` lists, restores,
purges, empties, and diagnoses that Trash. Neither command is a daemon, and RMU
does not maintain a private Trash database.

> [!WARNING]
> Three operations are permanent: `rmu -d` removes an empty directory,
> `rmu --delegate` hands the command to GNU `rm`, and confirmed `rmuctl purge`
> or `rmuctl empty` operations delete Trash contents.

## Why RMU?

- Familiar GNU `rm` options, ordering, prompts, and exit decisions.
- Standard home and per-volume Trash locations.
- Explicit `--delegate` escape hatch for exact GNU deletion behavior.
- Optional item/byte progress, transfer rate, ETA, elapsed time, and summary.
- Index-free Trash management and diagnostics through `rmuctl`.
- Static x86-64 and AArch64 binaries, RPM/DEB packages, manual pages, and
  Bash, Zsh, and Fish completions.
- No third-party Rust crates.

## Install

Release 0.7.2 provides RPM, DEB, and static musl archives for x86-64 and
AArch64 Linux on [GitHub Releases](https://github.com/UglyEgg/rmu/releases).
Download the package and its checksum manifest, then verify it before install:

```sh
sha256sum -c --ignore-missing PACKAGE-SHA256SUMS

# Fedora/RHEL, x86-64
sudo dnf install ./rmu-0.7.2-1.x86_64.rpm

# Debian/Ubuntu, x86-64
sudo apt install ./rmu_0.7.2-1_amd64.deb
```

The AArch64 package names end in `.aarch64.rpm` and `_arm64.deb`. Packages
install both commands, their manuals, and completions in the distribution's
standard vendor directories.

The static archive works without a package manager:

```sh
sha256sum -c --ignore-missing RELEASE-SHA256SUMS
tar -xzf rmu-0.7.2-x86_64-linux-musl.tar.gz
sudo install -m 0755 \
  rmu-0.7.2-x86_64-linux-musl/bin/rmu \
  rmu-0.7.2-x86_64-linux-musl/bin/rmuctl \
  /usr/local/bin/
```

To build from source:

```sh
cargo build --release
cargo test --all-targets --locked
```

Source builds support Rust 1.85 and newer. The checked-in development and
release toolchain is Rust 1.97.1. Containerized static and package builds are
covered in the [release engineering guide](docs/releasing.md).

## Try it safely

Start with a disposable file:

```sh
touch rmu-demo
rmu rmu-demo
rmuctl list
rmuctl restore ENTRY-ID
```

Confirm that the file appears in your desktop Trash and restores correctly.
If you then want interactive `rm` commands to use Trash by default:

```sh
alias rm='rmu'
```

Aliases normally do not affect scripts that call `/usr/bin/rm` directly. Use
`type -a rm` to see what your shell will execute.

## Use `rmu`

```sh
rmu notes.txt                         # Trash a file
rmu -r old-project                    # Trash a directory as one item
rmu -i important.txt                  # Ask before moving
rmu --rmu-progress -r large-tree      # Show progress on a terminal
rmu --delegate -rf build              # Run verified GNU rm permanently
```

Trash mode is the default. Select a mode explicitly with `--trash` or
`--delegate`, or set the default for an environment:

```sh
RMU_MODE=delegate rmu FILE
```

The command-line selector wins over `RMU_MODE`; when more than one selector is
present, the last one wins. Delegate mode verifies GNU coreutils `rm`, removes
only RMU's mode selector, and replaces the RMU process with that executable.

RMU accepts GNU's documented `-f`, `-i`, `-I`, `--interactive`, `-d`,
`--one-file-system`, `--preserve-root`, `--no-preserve-root`, `-r`/`-R`,
`--recursive`, and `-v` options. Grouped short options, `--`, permitted long
option abbreviations, non-UTF-8 names, and `POSIXLY_CORRECT` ordering are tested
against GNU coreutils `rm`.

### Progress

Progress is off unless requested or configured:

```sh
export RMU_PROGRESS=auto       # never, auto, or always
rmu -r large-tree
```

`--rmu-progress=never|auto|always` overrides the environment. When enabled,
recursive work reports item and allocated-byte rates, ETA when a complete total
is available, elapsed time, failures, and skipped operands.

### Empty directories

Like GNU `rm -d`, `rmu -d` permanently removes an empty directory with the
kernel's atomic directory-removal operation. Use `rmu -r` if the empty
directory and its metadata should remain recoverable. A Trash failure never
falls back to permanent deletion.

## Manage Trash with `rmuctl`

| Command | Purpose |
| --- | --- |
| `rmuctl list` | List entries and stable IDs |
| `rmuctl info ID` | Inspect entries |
| `rmuctl restore ID` | Restore without overwriting an existing path |
| `rmuctl purge ID` | Permanently delete selected entries |
| `rmuctl empty` | Permanently delete matching Trash entries |
| `rmuctl doctor` | Diagnose Trash structure; read-only by default |
| `rmuctl completions SHELL COMMAND` | Print a completion definition |

Preview permanent management work first:

```sh
rmuctl purge --dry-run ENTRY-ID
rmuctl empty --older-than=30d --dry-run
```

`purge` and `empty` require interactive confirmation or an explicit `--yes`.
No environment variable can authorize deletion, repair, recovery, overwrite,
or parent-directory creation. Trash quotas and automatic cleanup remain the
desktop environment's responsibility.

For a custom completion location, print the selected definition to standard
output:

```sh
rmuctl completions zsh rmu > /path/in/your/fpath/_rmu
rmuctl completions zsh rmuctl > /path/in/your/fpath/_rmuctl
```

The accepted shells are `bash`, `zsh`, and `fish`.

## Environment

<!-- markdownlint-disable MD013 -->

| Variable | Values | Purpose |
| --- | --- | --- |
| `RMU_MODE` | `trash`, `delegate` | Default `rmu` mode |
| `RMU_PROGRESS` | `never`, `auto`, `always` | Progress for `rmu`, purge, and empty |
| `RMU_GNU_RM` | executable path | GNU `rm` used for delegation and tests |
| `XDG_DATA_HOME` | directory | Home Trash base |
| `HOME` | directory | Fallback home Trash base |

<!-- markdownlint-enable MD013 -->

## Limits

RMU uses atomic rename and therefore requires the selected Trash to be on the
same mounted filesystem as the operand. It does not copy across filesystems,
provide GIO/GVfs Trash for remote URIs, or follow symlinks while scanning.
Recursive directory operands become one recoverable Trash item rather than a
partially deleted tree. Linux with `renameat2` support is required.

See the [GNU compatibility contract](docs/compatibility.md) for exact semantic
differences and the [architecture and safety model](docs/architecture.md) for
filesystem invariants.

## Documentation and support

- [Architecture and safety model](docs/architecture.md)
- [GNU `rm` compatibility contract](docs/compatibility.md)
- [Performance and memory benchmarks](docs/benchmarks.md)
- [Release engineering](docs/releasing.md)
- [Changelog](docs/CHANGELOG.md)
- [Contributing](docs/CONTRIBUTING.md)
- [Security policy](docs/SECURITY.md)

Bug reports and focused feature requests are welcome through the repository's
issue forms. Use disposable reproduction data and remove sensitive paths.
Potential vulnerabilities or data-loss paths belong in GitHub's private
vulnerability-reporting channel described by the security policy.

## License

RMU is available under the [MIT License](LICENSE). Static distributions carry
the required Rust 1.97.1 and musl 1.2.5 upstream notices.
