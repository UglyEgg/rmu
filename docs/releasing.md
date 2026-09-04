# Building and releasing RMU

## Local release artifacts

The builders use the Rust version pinned by `rust-toolchain.toml` in a
digest-pinned official Alpine image and request
exact Alpine package versions from the image's live repositories. Podman is
preferred when available; Docker is the fallback. Rust 1.85 remains the
separately tested source-compatibility floor; it must not be used to produce
release artifacts. Exact package availability in those live repositories is
not guaranteed indefinitely.

```sh
tools/build-musl-container.sh
tools/build-packages-container.sh \
  --maintainer 'Your Name <you@example.com>'
tools/test-packages-container.sh
tools/build-release-bundles.sh
```

The first command runs all release-mode tests and exports statically linked
`rmu` and `rmuctl` binaries plus checksums and provenance. The second wraps
those binaries in RPM and DEB packages, validates manuals and Bash/Zsh/Fish
completions, unpacks both packages, and checks that their executables are
byte-identical to the tested musl build. The package smoke-test command installs
and exercises those packages in digest-pinned Fedora and Debian containers,
then uninstalls them. The final command makes normalized binary and source
archives. Two no-cache builds on the same native runner are compared byte for
byte; that establishes same-environment repeatability, not indefinite or
cross-host reproducibility.

`release-files.txt` is the canonical top-level source/archive inventory and is
checked against both Git and `.dockerignore` before a release build.
`release-metadata.env` contains the canonical reproducible-build epoch; the
Rust version and matching upstream notice names are derived from
`rust-toolchain.toml`. Manifest generation stages and checks traversal, sorting,
file types, and hashing separately so an incomplete pipeline cannot be accepted
as a complete source identity.

The local builders produce native x86-64 or AArch64 artifacts according to the
container host. GitHub CI runs the complete release path on separate native
x86-64 and AArch64 runners. Each native job seals its outputs to the workflow's
immutable commit SHA. A single final job downloads both sets, revalidates their
embedded source identities and ELF architectures, confirms the remote tag
still resolves to that commit, and creates one complete draft release. It does
not update or replace assets in an existing draft.

The musl output's standalone `BUILD-INFO.txt` records the local container
engine and immutable artifact-image ID. Those two publication-specific fields
are omitted from the binary archive's copy so they do not create an avoidable
same-source difference. Its pinned base-image digest, toolchain/package
versions, complete source-manifest digest, runtime-source digest, Git HEAD, and
explicit clean/modified worktree state remain recorded. Both executables
also carry the complete and runtime source identities in a dedicated ELF
section, which the bundle verifier checks without executing the artifact. The
runtime identity covers compilation inputs only, so the checked-in benchmark
evidence can bind its measurements to the code without creating a documentation
hash cycle.

Default outputs are under:

- `outputs/musl/<architecture>/`
- `outputs/packages/<architecture>/`
- `outputs/release/<architecture>/`

Each builder publishes an immutable generation and atomically changes a
`current` link. Consume artifacts through the `current/` directory; the root
contains no per-artifact aliases that can become mixed across interrupted
publications. When reusing output from an older RMU builder, publication removes
only recognized links of the exact form `name -> current/name`; a regular file
or altered link is refused. Output directories are ignored by Git and must not
be committed.

## Release checklist

1. Update `Cargo.toml`, `Cargo.lock`, both manual headers, README, and
   `docs/CHANGELOG.md` to the same version.
2. Confirm `rustc --version` matches `rust-toolchain.toml`, `cargo tree
   --locked` lists only RMU, and `cargo +1.85.0 check --all-targets --locked`
   still passes.
3. Run formatting, all host tests, strict Clippy, the benchmark smoke test,
   both container builders, and `tools/test-packages-container.sh`. Confirm the
   GitHub CI release-path jobs pass on both x86-64 and AArch64.
4. Review the full change set against the compatibility, filesystem-safety,
   permanent-operation, artifact-identity, packaging, and documentation
   contracts. Resolve every release-blocking finding and retain the supporting
   test or build evidence.
5. Build the release bundles twice from unchanged source on the same native
   runner and compare archive and package checksums.
6. Inspect `BUILD-INFO.txt`, `PACKAGE-BUILD-INFO.txt`, all checksum manifests,
   RPM/DEB file lists, and the rendered manual pages.
7. Confirm that `origin` is the canonical GitHub repository, the default branch
   is `main`, Actions can create releases, and private vulnerability reporting
   is enabled. Confirm any backup remote separately; it is not a release source.
8. Confirm that public history contains no internal-only material, commit from
   a clean tree, and tag exactly `v<version>`. Sign the tag when the
   maintainer's public signing identity is available. Configure a GitHub tag
   ruleset that prevents updates and deletion of `v*` tags; draft releases are
   not protected by GitHub's immutable-release feature. The workflow rejects a
   tag whose version or peeled remote commit differs from the event commit.
9. Push the verified commit to GitHub and wait for both native CI paths. Then
   create and push the tag to GitHub. After the draft workflow succeeds, mirror
   the same immutable branch and tag refs to any backup remote.
10. Inspect the complete draft release and its mandatory safety preamble. Run
   `GITHUB_REPOSITORY=UglyEgg/rmu tools/verify-release-tag.sh v<version> <commit>`
   again immediately before publishing, and publish only after confirming the
   permanent `-d`, `purge`, and `empty` boundaries and the documented
   compatibility differences are accurate.

The release workflow uses checksum manifests but does not currently sign
packages or checksum files. Checksums detect corruption; they do not establish
publisher identity. Until signed provenance is added, consumers should obtain
artifacts from the canonical GitHub release and maintainers should use a signed
release tag whenever possible.
