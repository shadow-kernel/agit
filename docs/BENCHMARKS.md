# Benchmarks

Numbers here are measured, dated and reproducible. Anything not in this file
is a target, not a measurement.

## 2026-10-15: Phase 0 baseline

Machine: MacBook Pro, Apple Silicon (arm64), macOS 25.4, APFS on the internal
SSD. agit 0.1.0 built with `cargo build --release`. git 2.x from Xcode.

Workload: a tree of 2,000 small Rust files in 50 directories. Each iteration
changes one file, stages it and commits it. 200 iterations, wall clock per
iteration, shell loop spawning one process per command (so both tools pay the
same process start cost).

| Operation | git | agit |
| --- | ---: | ---: |
| `add` + `commit` of one changed file | 34.6 ms | 12.5 ms |
| `status` on a clean tree (50 runs) | 15.3 ms | 11.1 ms |
| `log -n 20` (50 runs) | 12.7 ms | 3.4 ms |
| on-disk size after 200 commits | 12 MB | 12 MB |

In-process (library, no process spawn), 3,000 iterations on the same tree,
`cargo run --release -p agit-core --example bench_commits -- 3000`:

| Durability | per `add`+`commit` | commits/s |
| --- | ---: | ---: |
| default (`AGIT_FSYNC=refs`, one fsync per commit) | 7.5 ms | 134 |
| `AGIT_FSYNC=none` | 2.7 ms | 370 |
| `AGIT_FSYNC=all` (about 7 fsyncs per commit) | 38 ms | 26 |

Per-step breakdown with `AGIT_FSYNC=none`
(`cargo run --release -p agit-core --example profile_commit`):

| Step | ms |
| --- | ---: |
| `add` one file (load index, hash, store blob, save index) | 0.99 |
| `commit` total | 1.69 |
| of which `write_tree` (51 tree objects re-encoded) | 0.95 |
| of which `diff_trees` (recursive, skips identical subtrees) | 0.17 |
| of which event append | 0.18 |
| `status` on 2,000 files (warm stat cache) | 8.0 |

### Addendum: cached trees in the index

After adding the tree cache (`AGI2` index format, directory tree ids cached and
invalidated per changed path), `write_tree` on a 2,000-file tree drops from
0.95 ms to 0.79 ms per commit; with nothing changed it costs 0.34 ms, which is
the cost of rebuilding the in-memory directory structure. The remaining time
goes into writing the three changed tree objects as loose files (temp file +
rename, about 100 us each on APFS) and loading/saving the index (about 0.3 ms
each). Both are what pack segments and a leaner index encoding address.

### Addendum: pack segments

Objects now go into append-only pack segments (one per writer process,
rolled every 1,024 objects) instead of one file per object; the segment
index is published atomically on flush, which happens before every ref
update and index save. Same workload as above:

| Durability | per `add`+`commit` | commits/s | before (loose objects) |
| --- | ---: | ---: | ---: |
| default (`AGIT_FSYNC=refs`) | 5.8 ms | 173 | 134 |
| `AGIT_FSYNC=none` | 3.4 ms | 298 | 370 |

With fsync the pack store wins because three temp-file-plus-rename
operations per commit are gone. Without fsync it is slightly slower than
loose objects because each flush rewrites the segment index (at most 1,024
entries, twice per commit); an append-only index is the next step. The
remaining 4 to 5 ms at default durability is the single `fsync` of the ref
file, which only a journal with group commit can amortize.

## What the numbers say

- The single ref fsync is the dominant cost at default durability. This is
  the exact cost that the Phase 1 journal with group commit amortizes: one
  fsync per batch instead of per commit.
- `write_tree` re-encodes every directory on every commit. A cached-tree
  extension in the index (only re-encode directories whose contents changed)
  removes most of that millisecond.
- `status` is bounded by the parallel directory walk and `lstat` calls, not
  by hashing. The million-file target in Phase 1 needs a persisted walk cache
  or filesystem monitor integration.

## How to reproduce

```sh
cargo build --release
cargo run --release -p agit-core --example bench_commits -- 3000
AGIT_FSYNC=none cargo run --release -p agit-core --example profile_commit
```

The CLI comparison is a shell loop; see the commit that added this file for
the exact script.
