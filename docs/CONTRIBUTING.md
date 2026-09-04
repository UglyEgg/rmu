# Contributing to RMU

RMU sits on a destructive-command boundary, even though its normal operation
is recoverable. Changes should preserve three promises: GNU `rm` command-line
decisions remain familiar, a failed Trash operation never falls back to
permanent deletion, and every delegate mode or permanent `rmuctl` action remains
explicit.

## Development setup

The source remains compatible with Rust 1.85, which CI checks separately.
Normal development and every release artifact use the repository-pinned Rust
1.97.1 toolchain. The Rust package has no third-party crate dependencies.

Install the two optional quality-check components once:

```sh
rustup component add --toolchain 1.97.1 rustfmt clippy
```

```sh
cargo fmt --all -- --check
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo tree --locked
```

The checked-in `rust-toolchain.toml` makes these commands select Rust 1.97.1.
Use `cargo +1.85.0 check --all-targets --locked` when explicitly checking the
minimum supported Rust version; do not use that compiler for release artifacts.
The last command above should show only `rmu`. GNU coreutils `rm` must be on `PATH`
for the differential tests; set `RMU_GNU_RM` to choose it explicitly.

Before submitting a release-sensitive change, also run the pinned container
builds:

```sh
tools/build-musl-container.sh
tools/build-packages-container.sh
tools/test-packages-container.sh
```

They choose Podman or Docker, run the release-mode test suite, validate static
linkage, lint the manuals and shell completions, build RPM and DEB packages,
compare the packaged executables byte-for-byte with the tested binaries, and
exercise package installation and removal in clean Fedora and Debian
containers.

## Change expectations

- Add a GNU-oracle case for parser, prompting, exit-status, or operand-order
  behavior.
- Add a deterministic race test when changing pathname or Trash publication
  logic.
- Keep arbitrary Unix path bytes out of unescaped terminal output.
- Keep `doctor` read-only by default and permanent actions confirmation-gated.
- Do not add a private Trash fallback or a third-party crate without first
  changing the project's explicit product constraints.
- Update both manual pages, completions, README, and changelog when the public
  interface changes.

Tests must use disposable roots. They must never discover or mutate the
developer's real desktop Trash.

## Reporting security-sensitive bugs

Do not open a public issue for a flaw that could delete, overwrite, restore, or
expose user data unexpectedly. Follow [SECURITY.md](SECURITY.md) instead.
