# RMU 0.7.2 release safety notes

## Safety boundaries

- `rmu -d` / `--dir` permanently removes an empty directory, matching GNU
  `rm -d`; it does not move that empty directory to Trash.
- `rmu --delegate` and `RMU_MODE=delegate` explicitly replace RMU with a
  verified GNU coreutils `rm`; all remaining arguments then have GNU rm's
  destructive behavior. Bare `rmu` and `--trash` remain recoverable by default.
- `rmuctl purge` and `rmuctl empty` permanently delete selected Trash entries.
  They require an interactive confirmation or an explicit `--yes`.
- RMU intentionally moves a recursive directory operand as one recoverable
  Trash entry. It cannot reproduce GNU `rm`'s partially deleted directory tree,
  and cross-filesystem copy fallback is not implemented.

Review the documented compatibility boundaries before aliasing `rm` to `rmu`.

Release checksum manifests detect accidental or malicious byte changes after
publication, but this release does not yet sign packages or checksum files.
Download artifacts only from the canonical `UglyEgg/rmu` GitHub release and
verify the matching SHA-256 manifest. Direct static binaries are accompanied by
the RMU, Rust, and musl license notices; the archive and packages carry those
notices internally.

The x86-64 and AArch64 artifacts are built and tested on native GitHub-hosted
runners. Both architectures must pass repeated static builds, RPM and DEB
creation, package installation and removal, command smoke tests, and release
archive comparison before one commit-bound draft is created.
