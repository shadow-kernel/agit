# agit architecture

> **Status note (2026-10-15).** This document was written before most of the platform existed. Where it marks components as planned, the current state is: pack segments (`objects/pack/seg-*.agp` with `AGX1` indexes, publish-on-flush), index format `AGI2` with a tree cache, a `refs.journal` with group commit and reflogs under `logs/`, Ed25519 signatures, the server with scoped tokens, policies, webhooks and the knowledge layer, the web UI, the MCP server and both SDKs are built. The byte layouts below are still accurate for `AGO1` loose objects, `AGT1` trees and commit JSON; the segment and journal formats are described in `docs/handbook/14-architecture-internals.md`.

This document describes how agit works today (Phase 0, `crates/agit-core` and `crates/agit-cli`) and how the later phases are designed. Everything marked ✅ is implemented and the byte layouts are taken from the code. Everything marked 📋 is a design and may change through the RFC process before it ships.

Legend: ✅ built, 🚧 in progress, 📋 planned.

## Contents

1. [Layered view](#layered-view)
2. [Object model](#object-model)
3. [Storage layout of `.agit/`](#storage-layout-of-agit)
4. [Concurrency model](#concurrency-model)
5. [Workspaces](#workspaces)
6. [Leases](#leases)
7. [Event log](#event-log)
8. [Merge](#merge)
9. [Checkpoints](#checkpoints)
10. [Planned pack and journal design](#planned-pack-and-journal-design)
11. [Planned server architecture](#planned-server-architecture)
12. [Knowledge base data model](#knowledge-base-data-model)
13. [Security model](#security-model)
14. [Format versioning](#format-versioning)

## Layered view

```
 +----------------------------------------------------------------------+
 |  Clients                                                             |
 |  agit CLI ✅   web UI 📋   Python SDK 📋   TypeScript SDK 📋   MCP 📋  |
 +----------------------------------------------------------------------+
 |  Public API (one surface for humans and agents)                      |
 |  local: `agit … --json` ✅        remote: HTTP/2 + gRPC 📋            |
 +----------------------------------------------------------------------+
 |  Platform services 📋                                                |
 |  PRs & reviews | policies | webhooks / SSE | knowledge index | auth   |
 +----------------------------------------------------------------------+
 |  Repository semantics ✅ (agit-core)                                  |
 |  commit | status | diff | merge | leases | workspaces | checkpoints   |
 |  revisions | events | ignore rules                                   |
 +----------------------------------------------------------------------+
 |  Storage primitives                                                  |
 |  object store ✅ (loose, AGO1)   pack segments 📋   journal 📋         |
 |  refs ✅ (files + CAS)           mmap ref table 📋                    |
 |  index ✅ (AGI1)   leases ✅ (JSON)   events ✅ (JSONL)   locks ✅      |
 +----------------------------------------------------------------------+
 |  Replication & hosting 📋                                            |
 |  Raft log per repo | blob tiers (disk, S3) | sharding by repo        |
 +----------------------------------------------------------------------+
```

The library (`agit-core`) has no notion of a server; the CLI is a thin layer over it. The server (📋) reuses the same object, ref and merge code, replacing only the storage primitives and adding replication.

## Object model

### Object ids ✅

```
id = BLAKE3-256( kind_tag:u8 || payload )
```

Kind tags: `blob = 1`, `tree = 2`, `commit = 3`. Ids are 32 bytes, rendered as 64 lowercase hex characters. The kind tag inside the hash guarantees that a blob and a tree with identical bytes have different ids, so a tree can never be confused for file content. `ObjectId::ZERO` (all zero bytes) is reserved and never a valid object.

Hashing of inputs of 128 KiB or more uses BLAKE3's multi-threaded mode (rayon). Prefix resolution (`agit rev-parse 7f3c…`) requires at least 4 hex characters and fails with `ambiguous_revision` when more than one object matches.

### Blob ✅

The payload is the file content, unmodified. No line-ending or encoding conversion.

### Tree ✅

Binary, little-endian, entries sorted bytewise by name, no duplicate names.

```
offset  size  field
0       4     magic "AGT1"
4       4     count:u32le           number of entries
8       …     entries, repeated `count` times:
        4     mode:u32le            0o100644 file, 0o100755 executable,
                                    0o120000 symlink, 0o040000 dir,
                                    0o160000 submodule
        32    id                    blob / tree / linked-repo commit id
        8     size:u64le            blob length; for dir: number of entries;
                                    for submodule: 0
        2     name_len:u16le
        n     name                  UTF-8, no '/', no NUL
```

Decoding rejects bad magic, unknown modes, truncated entries, non-UTF-8 names, trailing bytes, and unsorted entries (`corrupt_object`). Storing `size` in the entry is what lets `status`, `ls-tree` and the planned web UI answer size and type questions without opening a single blob.

A `submodule` entry's `id` is a commit id in a linked repository, not an object in this one. The URL and update policy live in `.agitmodules` (see [RFC 0007](rfcs/0007-submodules.md)).

### Commit ✅

Canonical JSON produced by serde in field declaration order; maps are `BTreeMap`, so key order is deterministic and encode(decode(x)) == x. Optional fields are omitted when absent rather than written as `null`; `checkpoint` is omitted when `false`; `meta` is omitted when empty.

| field | type | notes |
| --- | --- | --- |
| `tree` | id | root tree |
| `parents` | id[] | empty for a root commit; two or more for merges |
| `author` | `{name, email}` | |
| `time_ms` | i64 | Unix time in milliseconds, UTC |
| `message` | string | first line is the summary |
| `intent` | string? | why the change exists |
| `task` | string? | external ticket / issue id |
| `agent` | `{id, model?, run_id?, on_behalf_of?}`? | agent identity and provenance |
| `checkpoint` | bool | true for disposable checkpoints |
| `meta` | map string → JSON | namespaced extension point, e.g. `ci.tests` |

There is no separate committer field. An agent that rewrites history keeps the original `author` and records itself in `agent` or `meta`.

### Loose object file ✅

Path: `.agit/objects/<first 2 hex>/<remaining 62 hex>`.

```
offset  size  field
0       4     magic "AGO1"
4       1     kind:u8               1 blob, 2 tree, 3 commit
5       8     raw_len:u64le         uncompressed payload length
13      …     zstd frame            payload compressed at level 3
```

Write path: `tempfile` in the target directory, write header and zstd frame, `fdatasync`, `persist_noclobber` (rename). If the rename loses a race to another writer of the same object, the write is treated as success because the content is identical by construction. Writes are idempotent: an existing path short-circuits before any I/O.

Read path: parse header, decompress, check `raw_len`, re-hash `kind_tag || payload`, compare with the requested id. A mismatch is `hash_mismatch`; the data is not returned.

## Storage layout of `.agit/`

```
.agit/
  config.json            format_version, default_branch ("main"),
                         user{name,email}                                 ✅
  HEAD                   primary workspace: "ref: refs/heads/<b>\n" or
                         a detached 64-hex id                             ✅
  index                  AGI1 index of the primary workspace              ✅
  objects/xx/yyyy…       loose objects (AGO1)                             ✅
  refs/heads/<branch>    one file per ref, 64 hex + '\n'                  ✅
  refs/tags/<tag>                                                         ✅
  refs/checkpoints/<ws>                                                   ✅
  locks/refs.lock        flock targets (contents irrelevant)              ✅
  locks/leases.lock
  locks/events.lock
  leases/<id>.json       one file per active lease                        ✅
  events.jsonl           append-only event log                            ✅
  workspaces/<name>/     secondary workspace: HEAD, index, meta.json      ✅
  packs/*.seg            append-only pack segments + index                📋
  journal/               group-commit journal                             📋
  refs.table             memory-mapped ref table                          📋
  knowledge/             knowledge tree (notes, tag rules)                📋
```

The directory above is the *common directory*. The primary workspace's working tree contains it as `.agit/`; a secondary workspace's working tree contains a `.agit` **file** (JSON: `common_dir`, `workspace`) that points back to it. `.agit` is always ignored by `add` and `status`; `.agitignore` and `.gitignore` are both honored with gitignore semantics.

### Index ✅

```
offset  size  field
0       4     magic "AGI1"
4       …     bincode(Vec<IndexEntry>), entries sorted by path
-32     32    BLAKE3-256 of everything before it
```

`IndexEntry`: `path` (repo-relative, `/`-separated, UTF-8), `id`, `mode`, `size`, `mtime_ns` (i128), `ino` (u64), `conflict` (bool). The stat cache (`mtime_ns`, `ino`, `size`) lets `status` skip hashing files whose metadata is unchanged. After `switch` or `merge` the index is rebuilt from a tree with zeroed stat fields, so the next `status` re-hashes each file exactly once and then warms the cache. A checksum failure is reported as `invalid` with the path; the index is a cache and can be rebuilt from HEAD.

### Refs ✅

A ref is a file containing the full hex id and a newline. Names must start with `refs/`, must not end in `/` or `.lock`, must not contain `//`, `..`, control characters, or any of `space ~ ^ : ? * [ \`, and no path component may start with `.`.

## Concurrency model

### Locks ✅

Cross-process mutual exclusion uses advisory `flock` on files under `.agit/locks/`. The lock is held by an open file descriptor and released by the kernel when the process exits, however it exits. There is no sentinel file to clean up after a crash. Acquisition spins with exponential backoff from 1 ms to a 50 ms cap and gives up with `lock_timeout` (retryable) after 10 s for refs and 5 s for leases and events.

Three locks exist: `refs.lock` (all ref updates), `leases.lock` (claim, renew, release, purge), `events.lock` (append). Object writes take no lock: content addressing plus `persist_noclobber` makes concurrent writes of the same object safe without one.

### Compare-and-swap refs ✅

```rust
enum Expect { Any, Absent, Is(ObjectId) }
fn update(name, expect, new: Option<ObjectId>) -> Result<()>
```

Under `refs.lock`: read the current value, compare it to `expect`, then write (atomic temp + fsync + rename) or delete. A failed comparison is `ref_conflict { name, expected, actual }`, retryable. `Expect::Any` is last-writer-wins and is only used where the caller explicitly asks for it (`--force` paths); every normal commit, merge, branch creation and tag uses `Is(old)` or `Absent`.

### Multi-ref transactions ✅

`update_many(&[(name, expect, new)])` validates every expectation under the lock first (phase 1) and only then applies all writes (phase 2). Either every ref moves or none does. Because all ref writes go through the same single lock, there is no window where a reader sees a half-applied transaction across refs. Individual file renames are atomic, so a crash between two writes in phase 2 can leave the transaction partially applied on disk; this is the one known gap in Phase 0 and is closed by the journal in Phase 1, where a transaction is a single journal record.

### Idempotency ✅ / 📋

Object writes are idempotent by construction. Commit creation is idempotent in effect: the same tree, parents, author, time and metadata produce the same id, and a repeated ref CAS from the same old value to the same new value either succeeds or fails with `ref_conflict` whose `actual` already equals the intended value, which the CLI reports as success. Server-side idempotency keys (📋) extend this to network retries.

## Workspaces ✅

A workspace is a pair of `HEAD` and `index` that shares everything else in `.agit/`. The primary workspace created by `agit init` is named `main` and uses `.agit/HEAD` and `.agit/index`. `agit ws new <name> [--path dir] [--from rev] [--agent id]` creates a secondary workspace:

1. validates the name (no `/`, `\`, `.` or spaces, not `main`);
2. creates the working directory, by default the sibling `<primary-dir>.<name>`, which must be empty;
3. creates the branch `refs/heads/ws/<name>` at the start revision (default: the creating workspace's HEAD) with `Expect::Absent`, so each workspace begins on its own branch and two agents never race on one branch head by accident;
4. writes `.agit/workspaces/<name>/HEAD` (`ref: refs/heads/ws/<name>`) and `meta.json` (`name`, `path`, `created_ms`, `agent`);
5. writes the `.agit` pointer file into the new working directory and appends a `workspace.create` event.

`agit ws list` reads `meta.json` of each workspace. `agit ws remove <name>` deletes the registration under `.agit/workspaces/` and, with `--delete`, the working directory; it never touches objects or refs, and the `ws/<name>` branch stays until deleted explicitly.

Workspaces share objects, refs, leases and the event log, so a commit made in one workspace is immediately visible to `log` in another, and a lease claimed from one workspace binds commits in all of them. Events and leases carry an optional `workspace` field so a human (or the planned lease board) can see which agent is in which directory.

## Leases

### Data model ✅

`.agit/leases/<id>.json`:

```json
{
  "id": "a1b2c3d4e5f6",
  "holder": "coder-2",
  "paths": ["src/auth", "docs/auth/**"],
  "mode": "exclusive",
  "created_ms": 1760480000123,
  "expires_ms": 1760481800123,
  "note": "refactor login",
  "workspace": "coder-2"
}
```

`id` is the first 12 hex characters of a random UUIDv4. `mode` is `exclusive` or `shared`. `expires_ms` is absent for a lease without TTL (allowed, but discouraged). `note` and `workspace` are optional.

### Pattern coverage ✅

`pattern_covers(pattern, path)`:

- An empty pattern or `.` covers everything.
- A pattern without glob characters (`* ? [ {`) covers itself and everything beneath it: `src/auth` covers `src/auth` and `src/auth/login.rs`, not `src/authz/x.rs`.
- A glob pattern is compiled with `globset` and matched against the path and every ancestor of the path, so `src/*` covers `src/a/b` for leasing purposes even though a plain glob would not.

### Overlap ✅

`patterns_overlap(a, b)` is a conservative test used when at least one side is a glob: compare the literal prefixes (text before the first glob character); two patterns overlap if either literal prefix is empty, they are equal, one is a directory prefix of the other, or a glob's literal prefix is a plain string prefix of the other pattern (`src/foo*` vs `src/foobar/x`). False positives are acceptable (an agent is told to wait when it might not have needed to); false negatives are not.

### Claiming ✅

Under `leases.lock`: purge expired leases, then for an exclusive request scan all active exclusive leases held by a different holder and fail with `lease_conflict { path, holder, lease_id }` on the first overlap. The same holder may claim overlapping paths freely. Shared leases never conflict and are never conflicted against; they are informational.

### Expiry ✅

A lease is expired when `expires_ms <= now`. Expired leases are filtered out of every listing and physically removed on the next claim or explicit purge. There is no background process; expiry is evaluated lazily, which is sufficient because every decision that depends on leases re-reads them.

### Commit enforcement ✅

Before writing a commit, `agit commit` collects the paths changed relative to the parent tree and calls `find_conflict(holder, paths)`. The holder is the `--agent` id when given, otherwise the configured user. Any hit aborts the commit with `lease_conflict` unless `--force` is passed. Merge pre-checks use `changed_paths(base, theirs)` the same way. Enforcement is advisory in the sense that raw file writes are not intercepted; it is strict in the sense that the version control operation refuses to record the change.

## Event log ✅

`.agit/events.jsonl`, one JSON object per line:

```json
{"seq":122,"time_ms":1760480004417,"kind":"commit","workspace":"coder-2","actor":"coder-2","data":{"id":"9d2e…","branch":"main"}}
```

| field | notes |
| --- | --- |
| `seq` | monotonic within the repository, starts at 1 |
| `time_ms` | wall clock, milliseconds |
| `kind` | `repo.init`, `commit`, `checkpoint`, `restore`, `ref.update`, `head.switch`, `merge`, `lease.claim`, `lease.renew`, `lease.release`, `workspace.create`, `workspace.remove` |
| `workspace` | optional |
| `actor` | optional agent or user id |
| `data` | kind-specific JSON |

Appends take `events.lock`, read the last `seq` by scanning at most the final 64 KiB of the file, write one line with `O_APPEND`, and `fdatasync`. `read_after(seq)` scans the file and returns events with a larger sequence number; `--follow` polls the file tail and `--kind <prefix>` filters (`lease.` matches all lease events). A partial trailing line (crash mid-write) is skipped by the parser and overwritten by the next append's sequence logic. A sidecar counter and an index by `seq` are the obvious optimizations once logs grow; they do not change the format.

The `actor` of commit, checkpoint and merge events is the `--agent` id when given, otherwise the configured user name; the same value is used as the lease holder for enforcement.

## Merge

### Algorithm ✅

`merge_trees(base, ours, theirs)` flattens all three trees into `path → {id, mode, size}` maps and visits the union of paths in sorted order. Per path:

1. `ours == theirs`: keep it.
2. `ours == base`: take theirs. `theirs == base`: take ours.
3. One side deleted, the other modified: `delete_modify` conflict; the modified side is kept in the tree.
4. Both present, modes differ and neither equals the base mode: `mode` conflict.
5. Either side is a symlink: `content` conflict (symlinks are not text-merged).
6. Mode: whichever side changed it wins.
7. Same blob id after mode reconciliation: keep.
8. Any of the three blobs is binary (NUL byte in the first 8000 bytes): `binary` conflict, ours kept.
9. Otherwise a textual three-way merge (`diffy`) in **diff3** style. Success yields a new blob and the path is listed in `auto_merged`. Failure writes a blob with markers and records a `content` conflict, or `add_add` when there is no base.

Conflict markers:

```
<<<<<<< <ours label>
our lines
||||||| base
base lines
=======
their lines
>>>>>>> <theirs label>
```

Labels are the branch names (or ids) involved. The result is always a complete tree, so an agent can inspect the conflicted state with ordinary commands, and the index marks each conflicted path with `conflict: true`; `write_tree` refuses until all are cleared.

`MergeOutcome { tree, conflicts: [{path, kind, base?, ours?, theirs?}], auto_merged }` is what `agit merge --json` prints. Conflict kinds: `content`, `delete_modify`, `add_add`, `mode`, `binary`.

### Merge base ✅

Breadth-first walk from both commits in lockstep, recording the first commits seen from both sides. Candidates that are ancestors of other candidates are dropped; among the remaining, the one with the latest `time_ms` is chosen. Unrelated histories yield `None` and merge with an empty base (every differing path becomes `add_add`). The walk stops early once a candidate exists and the frontier exceeds 64 commits, a bound that trades theoretical optimality on pathological criss-cross histories for predictable cost.

## Checkpoints ✅

`agit checkpoint` snapshots the **whole working tree**, tracked and untracked files alike (ignore rules still apply), into a tree object, and writes a commit with:

- `checkpoint: true`;
- `parents: [HEAD]` (empty for a repository without commits), so a checkpoint is always a child of the real history, not of the previous checkpoint;
- `meta.agit.previous_checkpoint: <id>` linking to the previous tip of `refs/checkpoints/<workspace>`, which is how `agit checkpoint list` walks the chain newest-first;
- `message` defaulting to `checkpoint`, and `agent` when supplied.

The ref `refs/checkpoints/<workspace>` is then set with `Expect::Any` (checkpoints belong to one workspace, so there is no contention to detect) and a `checkpoint` event records `id`, `previous` and `files_changed` relative to HEAD. HEAD and the index are not touched, so an agent's branch state is exactly what it was before.

Because checkpoints hang off HEAD rather than off each other, they are never reachable from a branch and `agit log` never shows them; they share blobs and subtrees with the real commits, so the storage cost of ten checkpoints is the changed blobs plus ten small commits and trees.

`agit checkpoint restore <id>` (also `agit restore <rev>`, which accepts any commit) rewrites the working tree to the checkpoint's tree, including untracked files the checkpoint knew about, and appends a `restore` event. Promoting a checkpoint is a normal `add` plus `commit`. Planned (📋): `agit squash` to fold a checkpoint range into one commit, keeping the chain summary in `meta.agit.squashed`, and server-side compaction with the same semantics.

## Planned pack and journal design

📋 Phase 1. Target: more than 10k small commits per second on one laptop core, with the same durability guarantees as today.

### Where Phase 0 spends its time

A micro-commit that changes one file at depth three writes 1 blob, 3 trees and 1 commit (5 temp files and 5 renames), plus a ref write and an event append. With the default durability level (`durability::Durability::Refs`, the same policy git uses) only the ref write is fsynced: one fsync per commit. Measured on an Apple Silicon laptop that one fsync costs about 4.5 ms, which caps Phase 0 at roughly 130 to 200 commits per second per process regardless of CPU (about 370 per second with `AGIT_FSYNC=none`). `AGIT_FSYNC=all` syncs every object, index and log write as well, which costs about 7 fsyncs per commit. See `docs/BENCHMARKS.md`.

### Pack segments

Objects are appended to `packs/<segment-id>.seg`, an append-only file of `[kind:u8][raw_len:u64le][zstd frame]` records, with the object id stored in a sidecar index (`.idx`, sorted ids → segment offset) and mirrored in memory. A segment is sealed at a size threshold (for example 256 MiB) and then immutable; new writes go to a fresh segment. Lookup is: in-memory map, then sealed indexes (binary search or a small hash index), then loose objects for backwards compatibility. Loose objects remain the format for repositories below a few thousand objects, where simplicity wins.

### Journal and group commit

A commit becomes a single journal record: the new objects (inline for small ones, by segment offset for large ones), the ref CAS (name, expected, new), and the event. Records are appended to `journal/<n>.jnl`. A writer thread collects records for a batching window and issues **one** `fdatasync` for the batch, then acknowledges every commit in it. Multi-ref transactions are one record, which closes the partial-apply gap noted above. On startup, the journal is replayed into the pack and ref table; sealed journals are deleted after replay.

### Worked estimate

Assumptions: fsync latency 1 ms on a laptop NVMe (power-loss protected enterprise drives are closer to 0.1 ms); batching window 5 ms; a micro-commit adds about 2 KiB of compressed objects plus 200 bytes of record overhead; BLAKE3 at roughly 1 GiB/s per core; zstd level 3 at roughly 400 MiB/s per core.

- At 10,000 commits/s and a 5 ms window: 50 commits per batch, 200 batches/s, 200 fsyncs/s. Each batch waits at most 5 ms plus one fsync, so the median commit latency is about 3.5 ms and the tail about 6 ms.
- Bytes: 10,000 × 2.2 KiB ≈ 22 MiB/s of sequential append. Far below device bandwidth.
- CPU per commit: hashing 2 KiB at 1 GiB/s is 2 µs; compressing it at 400 MiB/s is 5 µs; JSON encoding and tree rebuild for a shallow path are in the same range. Budget 30 to 50 µs per commit, so one core has headroom to 20k commits/s before the journal becomes CPU-bound.
- The limiting factor at these rates becomes the ref CAS serialization point, which is why the ref table moves to memory with a mutex-protected CAS and periodic snapshot plus journal replay for durability.

These numbers are inputs to the design, not results. Phase 1 publishes measurements.

### Memory-mapped ref table

`refs.table`: a sorted array of `(name hash, name offset, id)` with a string arena, memory-mapped read-only by readers and rebuilt atomically (temp + rename) by the writer after every N journal batches. Live values come from the in-memory map that the journal keeps authoritative; the file is a snapshot for cold start and for external readers.

## Planned server architecture

📋 Phase 2 and 3. The server reuses `agit-core` for object encoding, merge and lease logic and replaces the storage primitives.

```
          HTTP/2 + gRPC (agit-protocol)
                    |
   +----------------+-----------------+
   |  gateway: auth, policies, routing by repo → shard            |
   +----------------+-----------------+
                    |
   +-------- shard (N repos) ---------+     +-------- shard ---------+
   |  per-repo Raft group             |     |                        |
   |   leader: accepts commits,       | ... |                        |
   |   appends to replicated log,     |     |                        |
   |   decides ref CAS at log head    |     |                        |
   |  followers: replay, serve reads  |     |                        |
   +----------------------------------+     +------------------------+
                    |
   +----------------+-----------------+
   |  blob store (content-addressed, dedup across repos)           |
   |   hot tier: local NVMe pack segments                          |
   |   cold tier: S3-compatible object storage                     |
   +----------------------------------------------------------------+
   |  event log → webhooks, SSE/WebSocket    knowledge index         |
   +----------------------------------------------------------------+
```

- **Commit log per repository.** Each push is a log entry containing the ref CAS set and the ids of the objects it introduces. Raft orders entries; the CAS is evaluated against the ref state at the log head when the entry is applied, so two agents racing for `main` get exactly one `ref_conflict`. Entries are batched per fsync, the same group-commit idea as the local journal.
- **Objects before log.** Clients upload objects first (deduplicated by id, verified by re-hashing on receipt), then submit the log entry. An entry referencing a missing object is rejected before it enters the log.
- **Blob tiers.** Recent and hot objects live in local pack segments on the shard; everything is also written to the S3-compatible tier, which is the durability anchor and makes shard rebuilds a matter of replaying the log and refetching.
- **Sharding by repository.** A repository lives on one shard; the gateway routes by repository id. Platform throughput scales with shard count. A hot repository is bounded by its single Raft group, hence the 10k to 20k commits/s per-shard target.
- **Protocol.** Negotiation (`have`/`want` by id), streaming pack transfer of segment-format records over HTTP/2 or gRPC, resumable uploads, idempotency keys on every mutating call, versioned message schemas. The wire format of an object is the same record format as a pack segment, so a push can be appended to a segment without re-encoding.
- **Events.** The applied log is the event source. The server's event stream has the same shape as the local `events.jsonl` plus repository and actor identity, delivered over SSE and WebSocket and to webhooks with at-least-once semantics and `seq`-based deduplication.
- **Pull requests and policies.** PRs are server-side structured objects keyed by repository; reviews and decisions append to the PR's own log. Policies run at log-apply time and at merge time ([RFC 0005](rfcs/0005-pull-requests-and-reviews.md)).

## Knowledge base data model

📋 Phase 4. Full design in [RFC 0003](rfcs/0003-knowledge-base.md).

A `note` is a fourth object kind (tag `4`), canonical JSON like a commit:

```json
{
  "title": "Session tokens are rotated on password change",
  "body": "…markdown…",
  "tags": ["area:auth", "kind:gotcha", "lang:rust"],
  "links": [
    {"type": "path", "value": "src/auth/session.rs"},
    {"type": "symbol", "value": "auth::session::rotate"},
    {"type": "commit", "value": "9d2e…"},
    {"type": "pr", "value": "42"}
  ],
  "valid_from": "9d2e…",
  "superseded_by": null,
  "author": {"name": "Daniel", "email": "daniel@example.com"},
  "agent": {"id": "reviewer-1", "model": "my-model-v3"},
  "confidence": 0.9,
  "time_ms": 1760480004417
}
```

- **Typed tags** are `namespace:value` strings. Reserved namespaces: `area`, `kind` (`decision`, `gotcha`, `spec`, `howto`, `rule`), `lang`, `service`. Unknown namespaces are allowed and indexed.
- **Tag rules** map path patterns to tags: `src/billing/** → area:billing, service:billing`. Rules answer two questions: which notes to read before touching a path (notes whose tags match the path's tags) and which tags to attach to a note written after changing that path.
- **Validity**: `valid_from` is the commit at which the note became true; `superseded_by` points to the newer note. Queries by commit range use both.
- **Storage**: notes and the rules file form a knowledge tree committed on `refs/knowledge/<branch>`, parallel to code branches, so knowledge merges with the same three-way machinery as code. The server indexes notes for tag, path and full-text queries; the local CLI falls back to scanning the tree.
- **Query results** are token-efficient: id, title, tags, confidence and a one-line summary first; bodies on demand.

## Security model

- **Verification on read ✅.** Every object read re-hashes the payload and compares it to the requested id. Trees reject malformed or unsorted input. The index carries a BLAKE3 checksum. A tampered or bit-rotted object cannot be served as valid data.
- **Ref name validation ✅.** Names are restricted so a ref can never escape `.agit/` or alias another ref via `..` or separators.
- **Agent identity ✅ (recorded), 📋 (authenticated).** Locally, `agent.id` is self-reported, like `user.name` in git. On the server, the token used for a push determines the asserted agent identity, and policies can require that `agent.id` in the commit match the token's principal.
- **Scoped, short-lived tokens 📋.** Tokens are issued per agent run with an explicit scope (repositories, ref patterns, operations) and a short lifetime, so a leaked token from a crashed run is useful for minutes, not months.
- **Policies 📋.** Evaluated server-side at log-apply and merge time: who may update which refs, required fields (`intent`, `task`), required checks, maximum files or bytes per commit, signed-off reviewers for protected branches. Policy decisions are recorded in the PR decision log and the event stream.
- **Leases are not a security boundary ✅.** They are a coordination tool. Nothing prevents a holder from passing `--force`; the force is recorded in the event stream so it is visible.

## Format versioning

- **Magic strings carry a version**: `AGO1`, `AGT1`, `AGI1`. A breaking change to a layout bumps the digit (`AGO2`) and readers keep decoding the old one until the 1.0 freeze declares what is supported.
- **Commit and note JSON evolve additively**: new optional fields are ignored by old readers because decoding does not reject unknown keys, and omitted by old writers. A field whose meaning changes gets a new name. Because the object id covers the exact bytes, re-encoding an old commit with a new writer would change its id; writers therefore never rewrite existing objects.
- **JSON output and errors** share a fixed envelope today (`{"ok": true, "result": …}` / `{"ok": false, "error": {"code", "message", "retryable"}}`). A per-schema `version` field inside `result` is planned (📋); it will increment only for breaking changes, and additions will not bump it.
- **Repository format version** is recorded in `config.json` (`format_version`, currently `1`) ✅ so a newer binary can refuse to open, or upgrade in place, a repository written by an older one.
- **Wire protocol** messages carry a protocol version negotiated at connection start.
- **Phase 5** freezes the on-disk and wire formats as 1.0 and publishes them under `spec/`. Until then, formats may change between minor versions with a migration path documented in the release notes.
