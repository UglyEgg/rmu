# Performance and memory benchmarks

RMU includes a local benchmark tool instead of making universal performance
claims. It compares GNU coreutils `rm`, `rmu --delegate`, and normal recoverable
Trash mode with progress disabled. Fixture creation and cleanup are outside the
timed region, mode order rotates, and one warm-up precedes each measured path.

Run it against a static release binary with:

```sh
tools/benchmark.sh \
  --rmu outputs/musl/x86_64/current/rmu \
  --gnu-rm /usr/bin/rm \
  --runs 20 \
  --tree-files 10000 \
  --output outputs/benchmarks/local
```

## RMU 0.7.1 reference run

- Date (UTC): 2026-09-08T10:35:21Z
- System: Linux 7.1.13-200.fc44.x86_64 x86_64 GNU/Linux
- CPU: AMD Ryzen 5 7640HS w/ Radeon 760M Graphics
- Temporary filesystem: btrfs
- RMU: rmu 0.7.1 (SHA-256 dda2a9aba4eb3223295e3232885c7a981c60c8be06582e381aa667df210207dc)
- Runtime source identity: `ec9f1661f684963fe88c0f5df0e30b16e7f52069539c2d71f59527f2f5d0d3fb`
- GNU rm: rm (GNU coreutils) 9.10 (SHA-256 af8e667181be4f25e27740c1c9ab16190b1b818bada52c42d0a1b3a450de8c7f)
- Measured runs: 20 per mode and scenario, after one warm-up
- Tree fixture: 10000 empty files in groups of 100
- Raw sample SHA-256: `d670d9714994c23e2ca479c772e87a3014ff21214ba469be8ab9c561c1b6be76`

The runtime identity is the SHA-256 of the compilation-input manifest embedded
in the measured binary. The exact [121-line raw sample set](benchmark-data/0.7.1/raw.tsv)
is checked in and verified against the digest and table below by CI.

<!-- markdownlint-disable MD013 -->

| Scenario | Mode | Median time (ms) | 5th-95th percentile (ms) | Median peak RSS (MiB) | Max RSS (MiB) | Time vs GNU rm |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| single | gnu-rm | 4.071 | 2.715-4.985 | 1.629 | 1.633 | 1.00x |
| single | delegate | 6.373 | 4.968-7.706 | 1.695 | 1.758 | 1.57x |
| single | trash | 5.970 | 3.711-6.890 | 0.689 | 0.816 | 1.47x |
| tree | gnu-rm | 643.556 | 531.103-818.862 | 1.570 | 1.758 | 1.00x |
| tree | delegate | 668.114 | 546.467-746.974 | 1.746 | 1.758 | 1.04x |
| tree | trash | 89.950 | 29.780-179.282 | 0.635 | 0.816 | 0.14x |

<!-- markdownlint-enable MD013 -->

The single fixture is one 4 KiB file. The tree fixture contains 10,000 empty
files grouped into 100 directories. Normal warm filesystem caches are retained;
the benchmark neither requests root access nor drops system caches.

GNU `rm` and `rmu --delegate` perform the same unlink operation. Securely
identifying and pinning GNU `rm` adds a fixed startup cost that is visible in
the single-file case; tree delegation was effectively identical to GNU `rm`.
The median Trash-mode tree run returned about 7.2 times faster because it did
not unlink every descendant. That is user-observed command latency, not
identical storage work: the trashed payload still occupies space.

Peak RSS remained below 1.9 MiB in every measured path. Filesystem, storage,
cache state, tree shape, security policy, and toolchain can materially change
both timing and memory, so rerun the tool on any target environment that matters.
