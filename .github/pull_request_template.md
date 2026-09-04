# Pull request

## Purpose

Describe the user-visible problem and the narrow change that solves it.

## Safety and compatibility

- [ ] No failed Trash operation falls back to permanent deletion.
- [ ] Permanent actions remain explicit and confirmation-gated.
- [ ] GNU-facing behavior has a differential oracle test, or the intentional
      difference is documented.
- [ ] Path and race-sensitive changes have deterministic coverage.
- [ ] Manuals, completions, README, and changelog match the public interface.
- [ ] No third-party Rust crate was introduced.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test --all-targets --locked`
- [ ] `cargo clippy --all-targets --locked -- -D warnings`
- [ ] Static musl builder
- [ ] RPM and DEB builder

## Release-sensitive changes

- [ ] Compatibility, filesystem safety, permanent operations, artifact
      identity, packaging, and documentation were reviewed as one change set.
- [ ] Release evidence identifies the tested commit and architecture.
- [ ] Any residual risk or unverified external gate is stated explicitly.
