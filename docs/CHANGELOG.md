# Changelog

## 0.7.2 - 2026-09-08

- Make embedded build-identity verification architecture-neutral by reading
  the ELF section with `readelf` instead of host-native `objcopy`, and test
  that verification succeeds when `objcopy` is unavailable. This lets the
  x86-64 release assembler validate sealed AArch64 binaries without weakening
  the commit, source-manifest, runtime-manifest, or ELF-architecture checks.
- Supersede the protected but unreleased `v0.7.1` tag after its final assembly
  gate exposed that cross-architecture tooling error. No 0.7.1 GitHub release
  or draft was published.

## 0.7.1 - 2026-09-04

- Require a shared `.Trash` directory to be sticky and owned by root or the
  current user, and secure RMU-managed Trash roots plus their `files` and
  `info` directories to mode 0700 before publishing private names or metadata.
- Revalidate source, Trash-root, and restore-destination namespaces at the
  transaction boundaries; roll verified data back when a directory moves
  instead of reporting stale-path success.
- Copy a selected musl generation into private staging before verification and
  consumption, and publish release output only below owner-controlled,
  non-symlink directory chains through one atomic `current` generation switch.
- Retire recognized legacy artifact links during publication while refusing
  regular files or links whose targets do not exactly match their old layout.
- Bound GNU `rm` verification output to 64 KiB and two seconds, then terminate
  and reap an unresponsive candidate instead of letting startup hang.
- Replace benchmark report and raw-sample symlinks as leaf entries rather than
  following them, and report medians, 5th-95th percentiles, and median/max RSS.
- Bound decoded `directorysizes` cache records as well as serialized bytes,
  batch one cache rewrite per Trash root, avoid per-node fdinfo reads during
  purge, and reserve collision names without a cumulative linear scan.
- Fail closed instead of falling back between per-volume Trash locations when
  an earlier transaction may have left unresolved metadata or payload residue.
- Make doctor distinguish initial observations from unresolved post-repair
  state, reinventory after recovery, and refuse partial `directorysizes`
  publication when a selected directory changes or cannot be counted.
- Correlate doctor observations across repair with typed problem identities,
  and reuse one parsed mount-table snapshot for discovery and root selection.
- Preserve typed post-rename capture and recovery state through rollback, and
  pin the verified GNU `rm` executable through delegation so path replacement
  cannot change what is executed.
- Bound hard-link accounting at 65,536 distinct multiply-linked identities,
  use one enumeration descriptor per open directory depth, time-throttle
  progress rendering, index stable IDs once, and batch cache rewrites per root.
- Split management discovery, actions, doctor/recovery, and transaction state
  into focused modules; centralize effective-access checks and restrict the raw
  Linux ABI layer to the x86-64 and AArch64 targets actually validated. Expose
  only the two command entry points from the otherwise private Rust crate.
- Replace duplicated release input lists, toolchain metadata, container-engine
  selection, and generation cleanup with checked canonical helpers. Reject
  special source nodes and upstream manifest failures, retain the prior output
  generation, and verify draft assets by exact names and bytes before leaving a
  release draft in place.
- Match GNU's exact-only `--no-preserve-root` spelling and document Trash
  mode's deliberate continue-after-output-failure behavior.
- Treat every indeterminate Trash-root inspection as incomplete, continue
  checking the private per-volume fallback, and block all `doctor` mutations
  when the global inventory is incomplete.
- Exclude only positively non-user-owned, non-searchable mounts from management
  scope; inaccessible user-owned mounts remain unknown and fail closed.
- Report failed transaction-owned metadata cleanup, centralize the 64 MiB
  `directorysizes` publication limit, and refuse an oversized rewrite before
  creating its temporary file.
- Bind restore, purge, empty, and mutating doctor operations to one complete,
  manager-issued inventory; capture and verify the whole current entry before
  acting so a concurrently completed pair is never split or deleted.
- Roll a captured payload/metadata pair back to its original desktop-visible
  names when restore publication fails, while keeping entry IDs stable across
  RMU's own reversible capture renames.
- Classify every failed quarantine, rollback, and Trash-publication outcome by
  rechecking identity before claiming that selected data is recoverable.
- Embed the source-manifest identity in both static executables; derive ELF
  architecture and static linkage at every bundle-reuse boundary instead of
  trusting editable metadata.
- Build both native architectures into commit-addressed workflow artifacts and
  create one complete draft only after revalidating the peeled remote tag;
  remove the later mutable `--clobber` upload path.
- Install DEB Zsh completions in Debian's default vendor directory, test real
  shell discovery, and make the RPM own and remove its package license
  directory.
- Make `rmuctl empty` fail before mutation when Trash discovery is incomplete,
  close the post-validation `rmu -d` replacement race with quarantine and
  identity verification, and match GNU `rm -f` for missing paths expressed as
  `ENOTDIR` or trailing-slash non-directories.
- Include skipped operands in progress summaries, add a successful restore
  transaction test, and complete Zsh help/version options for generated
  `rmuctl` completions.
- Neutralize archive-affecting environment variables, verify extracted archive
  members against staging, and make release checksums name the assets that are
  actually uploaded.
- Separate metadata, cache, descriptor, and permanent-removal invariants into
  focused modules; centralize the Linux calendar ABI and remove unused scan
  state without changing the command contract.
- Pin normal development, CI, static-musl, RPM, DEB, and release builds to
  Rust 1.97.1 while retaining Rust 1.85 as a separately tested source
  compatibility floor. Refresh the immutable Alpine builder image and exact
  Rust standard-library notices together.
- Refresh the published GNU `rm` comparison benchmarks with the Rust 1.97.1
  static binary and retain the complete raw sample set plus source identity.
- Refocus the README on installation and everyday use, consolidate public
  engineering documentation under `docs/`, add release/status badges and
  structured bug/feature issue forms, automate pinned GitHub Actions updates,
  remove obsolete internal-only material, and keep one maintained benchmark
  summary with its raw samples.
- Remove RMU-managed scheduled retention: Trash quotas and automatic cleanup
  belong to the desktop environment, while `rmuctl empty` remains an explicit
  on-demand operation.
- Add `rmuctl completions SHELL COMMAND` for writing the Bash, Zsh, or Fish
  definition for either `rmu` or `rmuctl` to any user-selected location, while
  distribution packages continue using standard system vendor directories.
- Add clean Fedora and Debian package-install smoke tests and mark the crate as
  not for crates.io publication.
- Correct the AArch64 Linux `O_DIRECTORY` and `O_NOFOLLOW` ABI values, add
  architecture-specific regression tests, and publish native AArch64 static
  binaries, RPM and DEB packages, and release archives only after the complete
  native release-path validation passes.
- Reduce large-tree accounting to one descriptor-relative `statx` call per
  entry, retain the prior descriptor-based metadata path when `statx` is
  unavailable, and refresh the checked benchmark evidence.

## 0.7.0 - 2026-09-02

- Add explicit `--trash`/`--delegate` operation modes and the corresponding
  `RMU_MODE=trash|delegate` environment default. Bare `rmu` remains
  recoverable; delegate mode verifies and replaces itself with GNU coreutils
  `rm`, preserving GNU's exact parsing, output, signals, and exit behavior.
- Let command-line mode selectors override even an invalid environment value,
  preserve `--` and `POSIXLY_CORRECT` filename boundaries, and refuse a GNU rm
  override that resolves back to RMU.
- Add first-class `rmuctl retention` management for systemd user timers.
  `retention enable` creates and starts a safe 30-day daily policy in one
  command, with options for age, schedule, and randomized delay; `status`,
  `disable`, and `remove` complete the lifecycle.
- Generate the service and timer atomically, retain the stable invocation path
  across symlink-based upgrades, refuse to overwrite hand-written units, and
  remove only files carrying RMU's management marker.
- Keep scheduled emptying daemonless and explicit: enabling retention writes a
  visible `rmuctl empty --older-than=... --yes` command, while environment
  variables still cannot authorize permanent deletion.
- Extend the Bash, Zsh, and Fish completions, man page, and retention guide for
  the one-command workflow.

## 0.6.0 - 2026-09-02

- Rename the project to RMU (Recoverable Move Utility), with the `rmu` and
  `rmuctl` commands, the `RMU_PROGRESS` environment variable, and the
  `--rmu-progress` extension. The mnemonic is "rm you can undo."
- Preserve recognition of quarantine and cache-temporary names written by
  pre-release `rtrash` builds so `rmuctl doctor` can still recover them.
- Add Bash, Zsh, and Fish completion definitions.
- Add generated man pages and containerized static, RPM, and DEB release
  plumbing.
- Add a source-reviewed comparison with other Linux Trash commands, complete
  architecture/compatibility/retention/release documentation, contribution
  and security policies, GitHub issue and pull-request templates, and pinned
  CI/release workflows.
- Publish static builds, packages, and deterministic source/binary archives as
  atomic generations with checksums and build provenance.
- Honor GNU `POSIXLY_CORRECT` option ordering and emit changing progress during
  the initial tree-counting pass.
- Refuse permanent purge across existing or newly introduced mount boundaries,
  validate entry IDs before diagnostics, fail nonzero on incomplete metadata
  cleanup, and avoid rolling an unverified post-move pathname back over the
  source.
- Bind every reused musl bundle to the exact current source and executable
  version, validate fixed-shape manifests, make publication interruption-safe,
  and reproduce binaries, packages, and archives across clean builds and
  caller umasks without executing artifacts from potentially `noexec` staging
  directories.
- Correct optional-value and subcommand completion behavior in Zsh, preserve
  whitespace-containing Bash paths, and ship a mandatory release safety
  preamble in draft GitHub releases.

## 0.5.0 - 2026-09-02

- Add the daemonless `rtrashctl` companion with `list`, `info`, `restore`,
  `purge`, `empty`, and read-only-by-default `doctor` commands.
- Discover and validate the standard home and per-volume FreeDesktop Trash
  roots, including bind aliases, sticky shared roots, relative metadata paths,
  local deletion timestamps, and partial-inventory failure reporting.
- Give each Trash incarnation an index-free ID derived from the physical root,
  payload name, and payload/metadata identities; stale IDs cannot select a
  later reuse of the same name.
- Add confirmation-gated permanent purge/empty operations, no-overwrite
  restore and orphan recovery, dry runs, retention/filesystem filters, atomic
  cache repair, and identity-pinned iterative directory deletion.
- Add exact Unix-byte hexadecimal companions to JSON output and keep all
  recovery/rollback locations visible in failure diagnostics.
- Extend opt-in `rtrash` progress with exact totals and a rolling ETA for
  readable trees, truthful lower bounds when a scan is incomplete, item and
  byte rates, and a final item/byte/elapsed/failure summary. Default output
  remains unchanged when progress is disabled.
- Keep the project dependency-free and isolate every mutating management test
  from the user's real desktop Trash.
- Add a Podman-or-Docker Alpine builder that tests, verifies, and exports
  statically linked musl binaries with checksums and build provenance.

## 0.4.1 - 2026-09-02

- Make `-d` use the kernel's atomic empty-directory removal, matching GNU
  `rm` without creating a meaningless empty Trash payload. Use `-r` when the
  directory object and its metadata should remain recoverable.
- Keep the `-d` path independent of `HOME`, `XDG_DATA_HOME`, and Trash layout
  initialization, and restore it to the GNU differential oracle.
- Give permanent `-d` failures their own truthful diagnostic path, and document
  the final pathname race reproduced identically against GNU `rm -d`.

## 0.4.0 - 2026-09-01

- Rename the project, Cargo package, executable, diagnostics, documentation,
  test fixtures, and release artifacts from `trash-rm`/`trash` to `rtrash`.
- Rename the custom progress extension to
  `--rtrash-progress[=auto|always|never]`.
- Add `RTRASH_PROGRESS=auto|always|never` as an environment default, with the
  command-line progress option taking precedence.
- Lazily initialize the Trash backend so forced no-op invocations do not depend
  on `HOME` or `XDG_DATA_HOME`.
- Refuse to re-trash existing entries through symlink aliases of the Trash root.
- Retry `.Trash-<uid>` after any safe pre-move failure in shared
  `.Trash/<uid>`, not only layout creation failures.
- Bound generated payload names to the destination filesystem's `NAME_MAX`,
  preserving full original names in `.trashinfo`.
- Escape arbitrary filename bytes in all terminal output and use locale-aware
  interactive confirmation.
- Pin scanned directories so concurrent symlink replacement cannot redirect a
  progress or size-cache traversal.
- Accept `-d` syntax but fail closed for empty non-recursive directories rather
  than claim equivalence with the kernel's atomic `rmdir` decision.
- Add Cargo package exclusions for local release artifacts and accept GNU's
  `yes`, `no`, and `none` interactive-value aliases.
- Publish each Trash pair in strict FreeDesktop order: exclusively create,
  write, and sync `.trashinfo`, then atomically move the source without
  replacing an existing payload.
- Detect payload disappearance before reporting success and document the
  unavoidable boundary around concurrently emptying Trash, for which the
  FreeDesktop specification defines no shared lock.
- Pin `files` and `info` relative to one pinned Trash root so concurrent root
  path replacement cannot split payload and metadata.
- Make newly created Trash directories, `.trashinfo` files, and cache
  temporaries independent of the caller's umask.
- Reject non-regular `directorysizes` entries without blocking on FIFOs or
  devices, keeping the optional cache non-disruptive.
- Normalize public backend source paths before metadata generation and resolve
  future symlinked Trash roots before creating any layout.
- Select Linux open flags per architecture, correct 32-bit-musl `time_t`, and
  provide a non-GNU-libc confirmation fallback.
- Emit per-operand failures in processing order, consume prompt input as raw
  bytes, and handle a closed verbose-output pipe without panic or abandoned
  operands.
- Quarantine transaction-owned Trash cleanup targets before identity checks
  and unlinking so a concurrently replaced entry is never blindly removed.
- Verify exact `.trashinfo` bytes before and after optional cache work and
  repeat `--one-file-system` mount detection after scanning.
- Refuse same-device bind-mount sources that desktop clients map to the home
  Trash but that cannot be moved there atomically without copy fallback.
- Bound scanner descriptor use by directory depth rather than tree width and
  pin `directorysizes` as a regular file before opening it for reading.
- Make stdout, stderr, prompt, help, and version writes fallible so closed pipes
  never panic or abandon later operands.
- Discover the GNU `rm` oracle through `RTRASH_GNU_RM` or `PATH` rather than a
  fixed filesystem location.

## 0.3.0 - 2026-09-01

- Add FreeDesktop `directorysizes` entries for trashed directories using
  allocated bytes and the corresponding `.trashinfo` modification time.
- Rewrite the cache through a bounded temporary file and atomic rename, replace
  stale same-name entries, and keep cache failures non-destructive.
- Accept owner-controlled KDE-style `0755` Trash directories while continuing
  to reject any group/other-writable layout.
- Fall back from an unusable shared `.Trash/<uid>` slot to `.Trash-<uid>` as
  required by the FreeDesktop specification.
- Validate both per-volume layouts in isolated mounted filesystems and complete
  an isolated create/list/restore round trip with KDE's KIO Trash implementation.
- Harden the GNU oracle harness against a benign broken pipe when a subprocess
  exits before consuming unused prompt input.
- Document the preflight architecture required before Landlock can be enabled
  without weakening multi-operand `rm` compatibility.

## 0.2.0 - 2026-09-01

- Pin source-parent and Trash directories with Linux directory descriptors.
- Revalidate source device, inode, and file type before and after `renameat2`.
- Attempt an atomic rollback if post-move identity verification fails.
- Add deterministic leaf-swap and ancestor-swap race tests.
- Add an iterative, physical directory scanner that does not follow symlinks
  and does not double-count hard-linked inodes.
- Add `--trash-progress[=auto|always|never]` with scan and move status.
- Retain dependency-free builds and unchanged default GNU `rm` output behavior.

## 0.1.0 - 2026-09-01

- Add the dependency-free GNU `rm`-compatible argument foundation.
- Add the FreeDesktop home and per-mount Trash backend.
- Add collision-safe `.trashinfo` creation and atomic no-replace moves.
- Add GNU `/usr/bin/rm` differential-oracle tests.
