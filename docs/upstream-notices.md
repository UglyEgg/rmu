# Bundled upstream notices

RMU's release binaries are statically linked with Rust's standard library and
musl. The exact upstream notice files are preserved in [`licenses/`](../licenses/)
and shipped alongside the RMU license.

- `Rust-1.97.1-COPYRIGHT` and `Rust-1.97.1-LICENSE-MIT` are the exact files
  from Rust tag 1.97.1, commit
  `8bab26f4f68e0e26f0bb7960be334d5b520ea452`.
- `musl-1.2.5-COPYRIGHT` is the exact COPYRIGHT file from the musl v1.2.5 tag.
- `Rust-1.97.1-COPYRIGHT-library.html` is the exact standard-library notice
  shipped in the Rust 1.97.1 toolchain for that commit. Rust generates it from
  the in-tree license metadata and the complete standard-library dependency
  inventory, so it is a conservative superset of the crates linked into RMU's
  x86-64 musl binaries.

The generated standard-library notice has SHA-256
`0a65bb747c49c7bb816cbc7188319bd6e4e8d08091c1190b8a3c0971c47968ed`.
When the pinned Rust or musl version changes, update the notices, container
version assertions, this provenance record, and package-payload checks in the
same release.
