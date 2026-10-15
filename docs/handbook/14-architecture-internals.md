# 14. Architecture and internals

How agit is built: the on-disk formats, the rules that make concurrent use safe, the algorithms behind merge and blame, how the git bridge maps between the two worlds, and what the throughput design looks like. Magic strings and layouts below were checked against the 0.1.0 binaries and the source in `crates/agit-core/src/`; the normative description is [docs/ARCHITECTURE.md](../ARCHITECTURE.md) and [RFC 0001](../rfcs/0001-object-model.md). Where ARCHITECTURE.md still describes the Phase 0 state (loose objects only, `AGI1`), this chapter documents what the binaries actually write.

## Layered view

```
clients        agit CLI | agit mcp | web UI | Python SDK | TypeScript SDK | Claude Code hooks
contract       agit --json envelope                 HTTP /api/v1 (docs/API.md)
platform       agit-server: refs CAS, objects, PRs, policies, tokens, leases, events/SSE, webhooks, knowledge, keys
semantics      agit-core: commit, status, diff, merge, blame, stash, leases, workspaces, checkpoints, reflog, signing, submodules
storage        pack segments (AGP) + loose AGO1 | refs (files, CAS) | index (AGI2) | leases (JSON) | events (JSONL) | flock locks
planned        journal + group commit | mmap ref table | Raft log per repository | blob tiers | sharding
```

Crates: `agit-core` (library, no notion of a server), `agit-protocol` (API types, object stream, want/have walk, HTTP client, push/fetch/clone, submodule commands, local knowledge store), `agit-server` (axum), `agit-git-bridge` (import/export via the `git` CLI), `agit-cli` (clap layer, MCP server). The server reuses the core's object, ref, merge and lease code and replaces nothing yet; its repositories are bare core repositories under `<data-dir>/repos/`.

## Formats

### Object ids

`id = BLAKE3-256(kind_tag || payload)`, kind tags `1` blob, `2` tree, `3` commit (`4` reserved for notes). Inputs of 128 KiB or more are hashed with BLAKE3's multi-threaded mode. Rendered as 64 lowercase hex characters; `ObjectId::ZERO` is reserved.

### Trees: `AGT1`

```
offset  size  field
0       4     magic "AGT1"
4       4     count:u32le
8       …     entries, repeated `count` times:
        4     mode:u32le     0o100644 file, 0o100755 executable, 0o120000 symlink, 0o040000 dir, 0o160000 submodule
        32    id             blob / tree / linked-repository commit id
        8     size:u64le     blob length; dir: entry count; submodule: 0
        2     name_len:u16le
        n     name           UTF-8, no '/', no NUL
```

Entries sorted bytewise by name, unique. Decoding rejects bad magic, unknown modes, truncation, non-UTF-8 names, trailing bytes and unsorted entries (`corrupt_object`).

### Commits: canonical JSON

Serde in field declaration order with `BTreeMap` for `meta`, optional fields omitted, `checkpoint` omitted when false, `meta` omitted when empty, `signature` last and excluded from the bytes a signature covers. Field list in [5](05-concepts.md#commits-and-metadata). The id covers the exact bytes, including the signature when present, so signing a commit produces a different id than the unsigned commit would have.

### Loose objects: `AGO1` (legacy, still read)

```
0   4   magic "AGO1"
4   1   kind:u8          1 blob, 2 tree, 3 commit
5   8   raw_len:u64le
13  …   zstd frame (level 3)
```

Path `objects/<hex[0..2]>/<hex[2..]>`. Written with temp file, `fdatasync` (under `AGIT_FSYNC=all`), rename without clobber; a lost rename race against an identical object is success. Repositories from earlier versions are read transparently; `agit gc` repacks loose objects into segments.

### Pack segments: `AGP1` files with `AGX1` indexes

The default store since the pack-segment addendum in BENCHMARKS.md. Each writer process appends to its own segment `objects/pack/seg-<pid>-<rand8>.agp`, guarded by `seg-….lock` (flock, 1 s timeout):

```
segment file:  magic "AGP1" | records…
record:        kind:u8 | raw_len:u32le | comp_len:u32le | id:[u8;32] | zstd frame
index file:    magic "AGX1" | count:u32le | entries sorted by id | blake3 checksum:[u8;32]
index entry:   id:[u8;32] | offset:u64le | comp_len:u32le | raw_len:u32le | kind:u8
```

A segment is rolled after 1,024 objects or 256 MiB. Lookup order: this process's in-memory map, published indexes (rescanned when the pack directory's mtime changes), then loose objects.

### The publish-on-flush rule

Objects appended to a segment are visible to other processes **only after the segment's index is published** (`.idx` written to a temp file and renamed atomically). `Repository` calls `flush()` before every ref update and every index save, and the server's `RefStore` has the same hook, so a reference can never point at an object another reader cannot find. A crash before the flush leaves unreferenced bytes at the tail of a segment; the next flush rewrites the index from what the writer knows, and `gc` eventually drops the segment's dead space. This replaces per-object fsync+rename with one index rewrite per flush, which is why pack segments beat loose objects at default durability (5.8 ms vs 7.5 ms per `add`+`commit` in BENCHMARKS.md).

### Index: `AGI2`

```
0    4    magic "AGI2"            ("AGI1" files are still read)
4    …    bincode(IndexFile { entries: Vec<IndexEntry>, tree_cache: Vec<(String, ObjectId, u64)> })
-32  32   BLAKE3-256 of everything before it
```

`IndexEntry`: `path` (repo-relative, `/`-separated, UTF-8), `id`, `mode`, `size`, `mtime_ns` (i128), `ino`, `conflict`. Entries are kept in a `BTreeMap` so tree building is one ordered pass. `tree_cache` maps a directory (`""` for the root) to the id and entry count of its already-written tree object; any change under a directory drops its entry and those of its ancestors, so `write_tree` re-encodes only what changed (0.95 ms to 0.79 ms per commit on a 2,000-file tree; 0.34 ms when nothing changed). A checksum failure is reported as `invalid`; the index is a cache and is rebuilt from a tree by `switch`, `merge` and `restore`.

### Refs, reflogs, leases, events

- A ref is a file `refs/<…>` containing 64 hex characters and a newline, written by temp + fsync + rename. `HEAD` is `ref: refs/heads/<b>\n` or a detached id.
- Reflogs are `logs/<refname>` (and `logs/HEAD` per workspace): one line `<old> <new> <actor> <time_ms>\t<message>`, zeros for "absent".
- Leases are `leases/<id>.json`, `id` = first 12 hex of a UUIDv4.
- Events are `events.jsonl`, one JSON object per line with a monotonic `seq`; appends read the last 64 KiB to find the previous `seq`, write with `O_APPEND`, `fdatasync`.
- `config.json`: `format_version` (1), `default_branch`, `user`, `remotes`.
- Secondary workspaces: `workspaces/<name>/{HEAD,index,meta.json}` plus a `.agit` pointer file in the working directory.

## Durability levels

`AGIT_FSYNC` is read once per process:

| Level | Object writes | Ref/index/log writes | Use |
| --- | --- | --- | --- |
| `refs` (default) | atomic (append + published index, or temp + rename), no fsync | fsync | normal use; same policy as git |
| `all` | fsync every object write | fsync | paranoid; about 7 fsyncs per commit |
| `none` | none | none | throwaway repositories, CI |

Measured on an Apple Silicon laptop (BENCHMARKS.md): default 5.8 ms per `add`+`commit` (173/s), `none` 3.4 ms (298/s), `all` about 38 ms. The remaining cost at default durability is the single fsync of the ref file, which only a journal with group commit can amortise.

## Locking and concurrency

Cross-process mutual exclusion uses advisory `flock` on files under `.agit/locks/`: `refs.lock` (all ref updates, 10 s timeout), `leases.lock` (claim, renew, release, purge, 5 s), `events.lock` (append, 5 s), plus the per-segment lock. Acquisition spins with exponential backoff (1 ms to 50 ms) and gives up with `lock_timeout` (retryable). The kernel releases a lock when the process exits, so there is no sentinel to clean up. Object writes take no lock beyond the writer's own segment: content addressing makes concurrent writes of the same object safe.

Ref updates are compare-and-swap under `refs.lock`: read, compare with `Expect::{Any, Absent, Is(id)}`, write or delete. `update_many` validates every expectation first and only then applies all writes (either all refs move or none, as seen by readers, because all ref writes share the lock). A crash between two renames in the apply phase can leave a multi-ref transaction partially applied on disk; this is the one known durability gap and is closed by the journal (📋). Commit creation is idempotent in effect: the same inputs give the same id, and a repeated CAS whose `actual` already equals the intended value is reported as success.

The server serialises PR and knowledge file mutations per repository with a mutex (`write_lock`), uses the core's ref lock for refs, and broadcasts events to SSE subscribers through a channel after appending them to the log.

## Merge algorithm

`merge_trees(base, ours, theirs)` flattens all three trees into `path → {id, mode, size}` maps and visits the union of paths in sorted order:

1. `ours == theirs`: keep.
2. `ours == base`: take theirs; `theirs == base`: take ours.
3. One side deleted, the other modified: `delete_modify` conflict; the modified side is kept in the tree.
4. Both present, modes differ, neither equals the base mode: `mode` conflict.
5. Either side is a symlink: `content` conflict (symlinks are not text-merged).
6. Mode: whichever side changed it wins.
7. Same blob id after mode reconciliation: keep.
8. Any of the three blobs binary (NUL in the first 8,000 bytes): `binary` conflict, ours kept.
9. Otherwise a textual three-way merge (`diffy`) in **diff3** style: success adds the path to `auto_merged`; failure writes a blob with markers (`<<<<<<< ours`, `||||||| original`, `=======`, `>>>>>>> theirs`) and records a `content` conflict, or `add_add` when there is no base.

The result is always a complete tree, so an agent can inspect the conflicted state with ordinary commands; the index marks conflicted paths and `write_tree` refuses until they are cleared. The merge base is found by a breadth-first walk from both commits in lockstep, dropping candidates that are ancestors of other candidates and choosing the latest `time_ms` among the rest; the walk stops early once a candidate exists and the frontier exceeds 64 commits, trading optimality on pathological criss-cross histories for predictable cost. Unrelated histories merge with an empty base (every differing path becomes `add_add`). The server's PR merge, `revert`, `cherry-pick` and `stash apply` all use the same function with different base/ours/theirs choices.

## Blame algorithm

`history::blame(store, rev, path, max_commits)` walks the **first-parent** history of the commits that touched `path` (the same filter as `log --path --first-parent`), newest first, at most `max_commits` (default 500). Starting from the blamed revision's lines, each step diffs the file between a commit and its first parent with a line diff (`diffy`, Myers) and attributes lines that were inserted in that commit; lines that survive the whole walk belong to the oldest commit examined. When the limit stops the walk early, `truncated` is `true`. Limitation: because only first parents are followed, a line introduced on a side branch is attributed to the merge commit that brought it in, not to the original commit on the branch (visible in the walkthrough, where the merge commit `3bf5d89c` owns the lines from PR #1). The web UI's client-side blame uses the same approach over `GET /commits?path=` and `GET /tree/{id}?path=`.

## Git bridge mapping

`agit git import` runs `git rev-list`, `git cat-file --batch` and `git for-each-ref` on the source (no libgit2):

| git | agit |
| --- | --- |
| blob | blob (bytes unchanged) |
| tree entry modes 100644, 100755, 120000, 040000, 160000 | file, executable, symlink, dir, submodule; sizes filled in from the blobs |
| commit author/committer, message | `author`, `message`; the committer is dropped (there is no committer field) |
| trailers `Intent:`, `Task:`, `Agent:`, `Model:` | lifted into `intent`, `task`, `agent.id`, `agent.model` |
| SHA-1 id | `meta["git.sha"]`; mapping table `.agit/git-map.json` (git sha → agit id) for incremental re-imports and exports |
| branches, tags | `refs/heads/*`, `refs/tags/*` (`--branch` and `--no-tags` filter) |

`agit git export` writes agit history into a git repository through `git fast-import`, referencing already-known git commits by SHA from the mapping table so repeated exports are incremental (`reused` in the report). agit's own git history round-trips. Continuous two-way sync and serving the git protocol are 📋.

## Server internals

- One `RepoHandle` per repository: core `ObjectStore`, `RefStore` with a before-update hook that flushes the store (publish-on-flush), `EventLog`, `LeaseStore`, a broadcast channel for SSE, the write mutex, and a `KnowledgeCache` (lazily built BM25 index plus a cache of "paths changed between A and B" used for staleness, cleared when it grows past its cap).
- Push: `POST /objects/missing` (client asks which of its reachable objects the server lacks), `POST /objects` (stream of only those, each re-hashed on receipt), `POST /refs/update` with the CAS; the policy check runs inside the ref update, on every new commit the update introduces (`commit_closure` of old..new, bounded at 50,000 commits).
- Fetch: `POST /fetch {wants, haves}` walks commits reachable from `wants` and not from `haves` and streams commits, trees and blobs in frames of `kind | len:u32be | payload`.
- PR merge: three-way merge with the core algorithm on the server's bare repository, a merge commit authored by the merging agent, then a CAS on the target; on success the draft note is written and `pull.merge` plus `note.draft` are emitted.
- Webhook deliveries run on one background thread fed by a channel, so emitting an event never waits on the network.
- Authentication middleware resolves the bearer to a principal (root, scoped token, or anonymous root in open mode), checks the required access for the method and path, injects the resolved agent, and appends to `audit.jsonl`.

## Performance picture

A micro-commit changing one file at depth three writes 1 blob, 3 trees and 1 commit (one segment append each, one index publish), a ref file (fsynced), an index save and an event append. At default durability the ref fsync (about 4 to 5 ms on a laptop SSD) dominates everything else combined (about 1 ms). CLI against CLI on a 2,000-file tree: `add`+`commit` 12.5 ms vs git 34.6 ms, `status` 11.1 ms vs 15.3 ms, `log -n 20` 3.4 ms vs 12.7 ms. In-process: 173 commits/s at default durability, 298/s with `AGIT_FSYNC=none`. Method and raw numbers: [docs/BENCHMARKS.md](../BENCHMARKS.md).

## What is planned

- **Journal with group commit** ([RFC 0004](../rfcs/0004-throughput.md)): a commit becomes one journal record (objects inline or by segment offset, ref CAS set, event), appended to `journal/<n>.jnl`; a writer thread batches records (default 5 ms window) and issues one `fdatasync` per batch. Closes the multi-ref gap, targets more than 10k commits/s per core.
- **Memory-mapped ref table** (`refs.table`): in-memory CAS, periodic atomic snapshots, journal for durability.
- **Server**: per-repository Raft-replicated commit log with the CAS decided at the log head, hot NVMe segments plus an S3-compatible cold tier deduplicated across repositories, sharding by repository, HTTP/2 and gRPC pack streaming, idempotency keys, resumable uploads. Targets: 10k to 20k commits/s per shard, 100k platform-wide.
- **Knowledge objects** ([RFC 0003](../rfcs/0003-knowledge-base.md)): note objects (kind tag 4) in a knowledge tree on `refs/knowledge/<branch>`, merged with the same three-way machinery; symbol links; semantic search.
- **Pull requests** ([RFC 0005](../rfcs/0005-pull-requests-and-reviews.md)): squash and rebase strategies with provenance under `meta.agit.squashed`, applyable suggestions, a decision log, reviews pinned to revisions.
- **Submodules** ([RFC 0007](../rfcs/0007-submodules.md)): `update` policies, `foreach`, cross-repository PR sets that merge atomically.
- **Web UI** ([RFC 0006](../rfcs/0006-web-ui.md)): server-side rendering, symbol navigation.
- **Format freeze** (Phase 5): `AGO`, `AGT`, `AGI`, segments, commit and note JSON and the wire protocol declared 1.0 under `spec/` with a conformance suite.
