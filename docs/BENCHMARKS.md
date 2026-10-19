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

## 2026-10-19: one fsync per commit, and which fsync

Same laptop, same `bench_commits` workload (3,000 micro-commits on a
2,000-file tree, `cargo run --release -p agit-core --example bench_commits
-- 3000`), measured back to back on the same afternoon. Another session was
building a web site on the machine during these runs, so absolute numbers
are a little below the 2026-10-15 figures; the `AGIT_FSYNC=none` row is the
control and did not move between the two builds.

What the standard library's `File::sync_data` costs on this Mac, 200
appends to one file (`scratchpad/fsync/bench.rs`, plain `rustc -O`):

| Primitive | per call |
| --- | ---: |
| `File::sync_data` (std issues `F_FULLFSYNC` on macOS) | 1.82 ms |
| `fsync(2)` | 0.027 ms |
| `fcntl(F_FULLFSYNC)` | 2.44 ms |
| `File::sync_all` | 3.05 ms |

git syncs with `fsync(2)`; agit was flushing the drive cache on every
commit. `durability::sync_file` now picks the primitive per level: `refs`
(default) uses `fsync(2)` like git, the new `full` level keeps the full
flush, `all` flushes everything.

| Durability | before (one `F_FULLFSYNC` per commit) | after | per `add`+`commit` after |
| --- | ---: | ---: | ---: |
| `refs` (default) | 130 commits/s | **277 commits/s** (a repeat run: 295/s) | 3.6 ms |
| `full` (new; the old default behaviour) | n/a | 120 commits/s | 8.3 ms |
| `none` | 258 commits/s | 278 commits/s | 3.6 ms |

At the default level the commit path is now CPU-bound (it matches `none`
within noise); on Linux, where `fdatasync` already was a plain fsync, nothing
changes. The trade-off is documented in the handbook: `refs` survives a
process or kernel crash, `full` also survives a power loss on Apple hardware.

Per-step breakdown with `AGIT_FSYNC=none` on the same run (`profile_commit`):
`add` 1.20 ms (`load_index` 0.37), `commit` 2.14 ms (`write_tree` 0.52,
`diff_trees` 0.08, event 0.06). The index load and save and the segment
index rewrite on flush are the next targets; `status` on 2,000 files sits at
7.3 to 8.4 ms across these runs.

## 2026-10-19: agit against git, side by side

`python3 scripts/compare_git.py --real-repo https://github.com/BurntSushi/ripgrep.git`
runs the same workloads with both tools in a temporary directory, out of the
box (default durability, no tuning, one process per command for both). The
git remote is a local bare repository and the agit remote an agit-server on
127.0.0.1; nothing goes over the network except the read-only clone of
ripgrep. Apple M3 Pro, 11 cores, 18 GB, macOS (Darwin 25.4), APFS, agit
0.1.3, git 2.50.1 (Apple Git-155). Other programs were running on the
laptop (load average about 3), so treat single-digit differences as noise.

Single agent, synthetic tree of 5,000 files in 100 directories (about 2 KB
each), medians:

| Operation | git | agit |
| --- | ---: | ---: |
| `status`, clean tree | 18.6 ms | 16.8 ms |
| `add` + `commit` of one changed file (100 iterations) | 37.8 ms (26/s) | **18.2 ms (54/s)** |
| `log -n 20` | 10.8 ms | 8.1 ms |
| create a branch, switch to it and back | **29.7 ms** | 43.6 ms |
| snapshot a dirty tree (200 modified, 20 untracked files): `stash push -u` + `stash apply` vs `checkpoint` | 431.4 ms | **23.1 ms** |
| roll back to that snapshot: `reset --hard` + `clean -fd` + `stash apply` vs `restore <checkpoint>` | 317.6 ms | **142.4 ms** |

Both rollbacks restored the tree exactly in 10 of 10 rounds and left branch
history untouched.

Crash: `add -A` of 2,500 changed files killed with SIGKILL after 5 to 80 ms,
then the agent retries the operation, 20 trials:

| | git | agit |
| --- | ---: | ---: |
| killed mid-operation | 20 | 15 (the rest had already finished) |
| retry failed, manual cleanup needed | **20** (`index.lock: File exists`) | **0** |
| `fsck` afterwards | clean | clean |

Many agents in one repository, each in its own worktree (git) or workspace
(agit) on its own branch, 25 commits each, all at once:

| Agents | git commits/s (p95) | agit commits/s (p95) |
| ---: | ---: | ---: |
| 1 | 22.7 (40.9 ms) | **63.1 (16.5 ms)** |
| 4 | 67.9 (60.7 ms) | **144.6 (38.4 ms)** |
| 8 | 75.4 (106.3 ms) | **122.7 (101.4 ms)** |
| 16 | 72.2 (235.5 ms) | **91.7** (268.4 ms) |

No errors on either side. agit is faster at every size, but its throughput
falls after 4 agents and its tail latency at 16 agents is higher than git's:
the shared journal, event log and lease files serialize concurrent commits.
That is the next scaling target.

Two pairs of agents among 8 want the same file:

| | git | agit |
| --- | --- | --- |
| overlap noticed | at merge time, after both agents finished: 2 merge conflicts | before any edit: 2 lease claims refused, median 6.3 ms |
| merge conflicts when integrating | 2 | 0 |

Pushing every commit to a local remote (git: bare repository on disk; agit:
agit-server over HTTP on 127.0.0.1), 25 commits per agent:

| Agents | git commit+push/s (p50) | agit commit+push/s (p50) |
| ---: | ---: | ---: |
| 4 | 34.7 (105.8 ms) | **115.3 (34.1 ms)** |
| 8 | 40.8 (180.2 ms) | **99.4 (74.4 ms)** |
| 16 | 39.2 (372.6 ms) | **57.5 (261.4 ms)** |

History on a real repository (ripgrep: 2,287 commits, 237 files; `agit git
import` took 2.0 s):

| Operation | git | agit |
| --- | ---: | ---: |
| `status` | 12.7 ms | 13.6 ms |
| `log -n 20` | 10.6 ms | 8.9 ms |
| `log -n 20 -- <file>` | 11.2 ms | 11.6 ms |
| `blame` of the most changed file | **48.1 ms** | 270.9 ms |

Reading: agit is ahead where agents spend their time (commit, snapshot and
rollback, crash recovery, parallel commits and pushes, overlap detection) and
on par for status and log. git is clearly faster at `blame` (5.6x; agit's
blame re-diffs every version of the file along first-parent history) and at
branch switching on large trees (1.5x). Both are on the performance list.

### The Claude Code integration: cost per tool call

`python3 integrations/claude-code/bench_hooks.py` installs the hooks into a
throwaway repository, starts an agit-server and feeds each hook the JSON
Claude Code sends, 30 times:

| Hook | median |
| --- | ---: |
| SessionStart briefing | 86.7 ms |
| PreToolUse on Edit: lease check and context, first touch / cached | 188.9 ms / 129.9 ms |
| PreToolUse on Edit, blocked by another agent's lease (exit 2) | 125.5 ms |
| PostToolUse on Edit: claim a lease / lease already held | 119.6 ms / 115.0 ms |
| Stop: checkpoint of a changed tree | 114.7 ms |
| PreToolUse on Bash: commit guard | 18.6 ms |

One file edit costs about 245 ms of hook time (310 ms the first time a file is
touched), against model turns of several seconds; most of it is starting
`python3` for JSON handling, not agit. Moving the hook logic into the `agit`
binary would cut it to the cost of one agit process (about 6 ms).

A live run with two headless Claude Code agents in two workspaces of one
repository, hooks installed, both asked to add a function to the same file
(coder-2 started once coder-1 held the file): coder-1 read the context,
followed the `kind:gotcha` note (integer cents, half-up rounding), got its
lease from the hook, committed with `agit commit --intent` and released the
lease, 8 turns, 26 s; coder-2 saw coder-1's exclusive lease in its session
briefing and left the file alone, 2 turns, 8.5 s. No conflicting edit
happened. coder-2 did not wait for the lease to be released one second later;
there is no command for that yet.

## 2026-10-19: server load, in-process and against the Docker image

Same laptop (Apple M3 Pro, 11 cores, 18 GB), release build of commit
`293be88` plus `scripts/loadtest.sh`, default durability (`AGIT_FSYNC=refs`,
now `fsync(2)`), nothing else running.

In-process example, `cargo run --release -p agit-server --example load -- 8
100` (clients and server in one process, no HTTP client processes):

| Phase | 8 agents |
| --- | ---: |
| 1: commit locally + push, per micro-commit | 673 commits/s (was 95/s on 2026-10-15 with `F_FULLFSYNC`) |
| 2: CAS updates advancing one hot branch (173 `ref_conflict` retries) | 2,089 updates/s |
| 3: CAS updates on independent branches (group commit) | 3,565 updates/s |

Against the Docker image built from the checkout (`docker build -t agit:dev
.`, Docker Desktop 29.4, linux/arm64 VM with 11 CPUs and 9 GB, named
volume), closed with `AGIT_ADMIN_KEYS`, driven by `scripts/loadtest.sh`: N
agents, each in its own clone on its own branch, one `agit` process per
`add`, `commit` and `push`, over HTTP on localhost:

| Phase | 8 agents x 50 | 32 agents x 25 |
| --- | ---: | ---: |
| 1: `add` + `commit` + `push` per micro-commit | 102 commits/s | 115 commits/s (1 push of 800 failed) |
| 2: `push` only (commits made first) | 109 pushes/s | 125 pushes/s |
| 3: no-op `fetch` | 248 fetches/s | 229 fetches/s |

Reading: one `agit` process costs about 5.7 ms on this machine (`agit
status --json`, warm), and each micro-commit in phase 1 spawns three of
them plus the server round trips, so the HTTP numbers are bounded by
process spawn and the container's disk path, not by the server: the same
server does 673 commits/s when driven in-process. Agents that talk to the
server through the SDKs or the MCP server (one long-lived process) sit
between the two. The 32-agent run also showed that the audit log interleaved
lines from concurrent requests; `audit()` now writes each line with one
`write(2)`.

## 2026-10-15: server load test (single node)

`cargo run --release -p agit-server --example load -- <agents> <commits>`
starts an in-process server and drives it from many clients. Same laptop.

| Phase | 8 clients | 32 clients | 8 clients, `AGIT_FSYNC=none` |
| --- | ---: | ---: | ---: |
| 1: commit locally + push, per micro-commit (client and server on the same disk) | 95/s | 91/s | 687/s |
| 2: CAS updates advancing one hot branch (latency bound: every client re-reads after each success) | 344/s | 209/s | 1471/s |
| 3: CAS updates on independent branches (group commit) | 359/s | 405/s | 2140/s |

Reading: phase 1 is dominated by the clients' own fsyncs because they
share the laptop's disk with the server; in a deployment they are on other
machines. Phase 2 is bounded by one fsync latency per step by design.
Phase 3 is the server's own ref path: the journal with group commit makes
it scale with concurrent writers, and the `AGIT_FSYNC=none` column shows
the CPU ceiling of the current per-update work (flock, reflog, events,
audit, commit events) at about 2,000 updates per second on one core-ish
path. Reaching the 100k platform target needs sharding across repositories
and nodes plus a leaner per-update path; both are tracked in RFC 0004.

## What the numbers say

- The single journal fsync was the dominant cost at default durability, and
  on macOS most of it was the drive-cache flush that git does not do either.
  With `fsync(2)` the default level is CPU-bound; the journal's group commit
  still amortizes the remaining sync across concurrent writers, and `full`
  is there for power-loss safety.
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
