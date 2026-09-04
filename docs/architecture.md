# Architecture and safety model

RMU is two dependency-free Rust commands over one FreeDesktop Trash model.
`rmu` owns mode selection, GNU-compatible argument decisions, and recoverable movement;
`rmuctl` owns discovery, inventory, restoration, diagnosis, and completion output.
Neither command is a daemon and neither maintains a private index.

## Main components

<!-- markdownlint-disable MD013 -->

| Module | Responsibility |
| --- | --- |
| `mode` | Early Trash/delegate selection, selector removal, GNU rm verification, and exact process delegation |
| `args` | GNU-shaped `rmu` parsing, including order-sensitive prompt flags and namespaced progress |
| `engine` | Per-operand validation, prompting, result ordering, events, summaries, and exit decisions |
| `freedesktop` | Trash-root selection, metadata publication, fail-closed recovery, and cache updates |
| `freedesktop/fsops` | Descriptor-relative movement, quarantine, identity checks, and permanent removal |
| `scanner` | Iterative physical tree accounting, hard-link deduplication, mount detection, and progress events |
| `ctl_args` | `rmuctl` grammar, completion selection, and non-destructive environment policy |
| `rmu_cli` / `rmuctl_cli` | Command rendering, standard streams, and process exit boundaries |
| `control` | Management facade, inventory authorization, selection, and shared result types |
| `control/actions` | Restore and permanent purge operations |
| `control/metadata` | Bounded `.trashinfo` parsing, local deletion dates, and incarnation IDs |
| `control/cache` | Bounded `directorysizes` parsing and atomic cache replacement |
| `control/discovery` | Trash-root discovery, inventory inspection, filters, and path resolution |
| `control/doctor` | Diagnosis, cache repair, and deterministic orphan recovery |
| `control/transaction` | Capture, rollback, quarantine, and recovery state transitions |
| `directory_sizes` | Shared serialized-size limit for every cache reader and publisher |
| `linux` | x86-64/AArch64 ABI calls, directory iteration, access checks, signals, and C calendar conversion |
| `display` | Byte-preserving, terminal-safe path rendering |

<!-- markdownlint-enable MD013 -->

The Rust crate is an implementation detail of the two commands. Its only public
items are the two narrow binary entry points; filesystem engines, capability
types, parsers, and test seams remain private rather than implying a supported
library API.

Tree accounting obtains entry type, hard-link identity, and allocated blocks
with one descriptor-relative Linux `statx` call. Kernels that do not provide
that interface fall back once per scan to the previous `O_PATH` plus descriptor
metadata path. Both paths avoid following symbolic links, and directory
descriptors are still identity-checked after opening before traversal.

## Operation-mode boundary

Trash mode is the default. Before the GNU-shaped Trash parser runs, `mode`
recognizes exact `--trash` and `--delegate` selectors while preserving `--`
and `POSIXLY_CORRECT` boundaries. Command-line selectors override
`RMU_MODE=trash|delegate`, and the last selector wins. In delegate mode RMU
verifies a GNU coreutils `rm`, refuses a candidate resolving back to itself,
and replaces its process with GNU rm after removing only the mode selectors.
The identification probe has a two-second deadline and a 64 KiB output bound;
an unresponsive candidate is terminated and reaped. There is no attempt to
reproduce destructive traversal inside RMU.

## Trash transaction

For each recoverable operand, the backend selects the home or per-volume Trash
required by the FreeDesktop specification, validates ownership and write
permissions, and pins the source parent and Trash directories. It then:

1. revalidates the source device, inode, and type;
2. reserves a unique `.trashinfo` name with exclusive creation;
3. writes and syncs the encoded original path and deletion time;
4. moves the payload with descriptor-relative `renameat2(RENAME_NOREPLACE)`;
5. verifies the payload identity and exact metadata bytes;
6. queues a bounded `directorysizes` update for the root-level batch; and
7. reports success only after final identity checks.

If publication fails before movement, transaction-owned metadata is removed;
if safe removal itself fails, that recovery state is part of the returned
error rather than being hidden. A failed shared-Trash attempt may fall back to
the private per-volume Trash only when its transaction cleanup is proven
complete; unresolved residue fails closed.
If verification fails after movement, RMU leaves the uncertain Trash pathname
in place for explicit recovery rather than risk moving a substituted entry over
the source. Cleanup first quarantines and validates an entry so a concurrently
substituted pathname is never blindly unlinked.

`-d` is a separate path. It quarantines and verifies the selected directory by
identity before asking the kernel to remove it only if empty, and it never
initializes Trash. This is permanent by design and matches the documented GNU
option.

Permanent directory purge first refuses any mounted path at or below the
selected payload. The descriptor-backed remover also compares Linux mount IDs
for the root and every child before traversal. A bind mount introduced after
preflight is therefore refused rather than followed into another mounted view.

## Index-free management

`rmuctl` discovers valid Trash roots on each run, considering ordinary data
mount instances even when the mount table currently labels one read-only. One
parsed mount-table snapshot is shared across that invocation so all discovery
decisions use the same view.
Enumeration completeness and mutation success are separate facts: the pinned,
descriptor-relative filesystem operation is the final authority on whether a
selected root can be changed. An inaccessible mount
beneath a user-owned parent is unknown and fails closed; an inaccessible
system-owned mount is outside the calling user's manageable Trash scope. Entry
IDs combine the physical root identity, the desktop payload name, and a
rename-stable incarnation fingerprint derived from payload and metadata
identities. Mutating operations rebuild a complete inventory, bind the selected
entry to that inventory, reopen pinned roots, and independently revalidate its
identity immediately before capture. A previously printed ID is therefore a
selector, not permanent authority.

The lack of a private database is deliberate: desktop file managers remain the
other authoritative writers. Only a definite `NotFound` result counts as an
absent eligible Trash candidate. Every other inspection failure within the
manageable scope makes discovery incomplete, is surfaced as a warning and
nonzero exit, and blocks all mutating management operations. An incomplete
inventory is never reported as an
authoritative empty Trash or accepted by a permanent bulk operation. The
control API represents this boundary with an authorized-inventory capability;
restore, purge, empty, cache repair, and orphan recovery are available only
through a complete inventory issued by the same manager, and selected entries
must be borrowed from that inventory. Restore, purge, and orphan recovery share
one descriptor-relative snapshot transaction: expected components are captured
and identity-checked, then both original names must be absent before an action
continues. A concurrent writer therefore causes a verified rollback or an
explicit unknown-location result instead of acting on a stale orphan state.
Capture and recovery states remain typed until the diagnostic boundary, so a
post-rename failure cannot be mistaken for a component that was never moved.
When bind aliases expose one physical per-volume Trash through multiple mount
paths, relative metadata is not guessed: restore requires an explicit `--to`
destination.
Concurrent writers can make the advisory `directorysizes` cache stale through
last-writer-wins updates; payloads and metadata remain authoritative, and
`rmuctl doctor --repair-cache` rebuilds the cache. Every reader and publisher
enforces the same 64 MiB serialized-cache boundary. A cache read failure is
reported as a warning but does not make the authoritative payload/metadata
inventory incomplete. This lets `doctor` repair the advisory cache or recover
an independently identified orphan without using the unreadable cache as
evidence. Successful trash, restore, and purge batches rewrite each affected
root's cache once. Cache repair publishes nothing unless every selected
directory is revalidated and counted completely. After a repair, `doctor`
reinventories and bases its exit status on unresolved post-action state rather
than the original observations.

## Progress and accounting

Progress is an observer of the normal engine. When enabled for a recursive
operand, an initial count provides item and allocated-byte totals; the existing
size scan then emits rates and ETA. If either scan is incomplete, values are
labelled as lower bounds and ETA is suppressed. The final summary reports
items, allocated bytes, elapsed time, failed operands, and skipped operands
without changing the underlying success decision. Terminal redraws are
time-throttled while the observer still retains and flushes the latest state.

Tree accounting uses one descriptor per open depth and a bounded hard-link
identity set. It refuses an exact total before retaining more than 65,536
distinct multiply-linked file identities; the move can still proceed, but the
summary becomes an explicit lower bound and no untrusted directory-size cache
record is published.

The dependency-free raw ABI layer is intentionally compiled only for x86-64
and AArch64 Linux, matching the architectures built and tested by CI.

## Explicit non-goals in 0.7

- copying across filesystems during trash or restore;
- remote URI Trash through GIO/GVfs;
- a private graveyard or recovery journal;
- a resident daemon;
- Trash quotas, automatic emptying, or other desktop retention policy;
- following symlinks while scanning;
- silently weakening validation to increase apparent compatibility.
