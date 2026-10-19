> Status update 2026-10-19: Phase 0 is complete. Phase 1 is partly built (pack segments, the ref journal with group commit, the tree cache, measured benchmarks; the mmap ref table, million-file `status` and token-efficient views remain). Phases 3 and 4 shipped as a single-node server (`crates/agit-server`): clone/push/pull with CAS refs, pull requests with reviews and server-side merge, policies, webhooks, server-wide leases, SSE event stream, the knowledge base with BM25 search, staleness, drafts and context packs, submodules, the MCP server and Claude Code hooks, both SDKs, the web UI and the git bridge. Phase 2 has clone/push/pull and SSH-key plus scoped-token authentication with per-repository scopes; Raft replication, blob tiers and sharding remain open.

# agit roadmap

Six phases, each with concrete deliverables, acceptance criteria, and benchmark targets. A phase is done when every acceptance criterion is met and the benchmarks for that phase are published in this repository with the exact hardware and commands used.

Benchmark figures in this document are **targets**. None has been measured yet; the first published measurements arrive with Phase 1.

Legend: ✅ built, 🚧 in progress, 📋 planned.

| Phase | Theme | Status |
| --- | --- | --- |
| 0 | Local core | ✅ |
| 1 | Performance | 🚧 (packs, journal, tree cache, benchmarks ✅) |
| 2 | Protocol and server | 🚧 (single node with auth ✅; replication 📋) |
| 3 | Platform | ✅ |
| 4 | Knowledge base and AI integration | ✅ |
| 5 | Self-hosting and 1.0 | 📋 |

---

## Phase 0: Local core

A git-shaped local VCS that agents can drive safely from JSON, in Rust.

### Deliverables

- ✅ Object model: BLAKE3 ids, blob, `AGT1` tree with sizes and submodule mode, canonical-JSON commit with `intent`, `task`, `agent`, `checkpoint`, `meta`.
- ✅ Loose object store (`AGO1`, zstd, temp + fsync + rename, verify on read).
- ✅ Index (`AGI1`, bincode, BLAKE3 checksum, stat cache).
- ✅ Refs with compare-and-swap and multi-ref transactions; `flock` locks, no sentinel files.
- ✅ Leases: exclusive/shared, globs, TTL, conflict detection, commit enforcement.
- ✅ Event log with sequence numbers.
- ✅ Workspaces with their own HEAD and index.
- ✅ Checkpoints on `refs/checkpoints/<workspace>`.
- ✅ Diff (tree and unified), three-way merge with diff3 markers, merge base.
- ✅ CLI: `init, add, rm, reset, status, commit, checkpoint (list, restore), restore, log (--agent, --task, --path), show, diff, branch, switch/checkout, merge, tag, cat-file, ls-tree, lease (claim, release, renew, list, check), ws, events, rev-parse, fsck, config`, all with `--json` in the `{ok, result|error}` envelope and exit codes 0/1/3/75.
- 🚧 Test suite: unit tests per module, CLI integration tests covering every command in both output modes, crash-injection tests for object and ref writes.
- 🚧 `.agitignore` and `.gitignore` parity tests against a corpus of gitignore edge cases.

### Acceptance criteria

- Every command returns valid JSON with `--json` in the `{"ok": true, "result": …}` envelope, and every failure returns `{"ok": false, "error": {code, message, retryable}}` with a code from the documented list and the matching exit code (1, 3 or 75).
- Killing `agit` with SIGKILL at any point during `commit`, `merge`, `lease claim` or `checkpoint` leaves a repository that `agit fsck` reports as clean and that needs no manual cleanup. No `.lock` files exist anywhere.
- Two processes committing concurrently to the same branch: exactly one succeeds, the other receives `ref_conflict` with `retryable: true` and succeeds on retry after re-reading.
- A commit touching a path under another holder's active exclusive lease fails with `lease_conflict`; the same commit succeeds after the lease expires or is released, or with `--force`.
- `cat-file` on a bit-flipped object returns `hash_mismatch`, never the corrupted bytes.
- `encode(decode(x)) == x` for every object kind, verified by property tests.

### Benchmark targets

None published in this phase. A baseline measurement of loose-object commit throughput and `status` on 100k files is recorded as the reference point for Phase 1.

---

## Phase 1: Performance

Make micro-commits cheap locally and publish honest numbers against git.

### Deliverables

- Append-only pack segments (`packs/*.seg`) with on-disk and in-memory indexes; loose objects kept for small repositories; `agit gc` to repack.
- Journal (`journal/`) with group commit: one `fdatasync` per batch; commits, multi-ref transactions and events as single journal records; replay on startup.
- Memory-mapped ref table with in-memory CAS and periodic snapshots.
- Parallel `status` and `add` using the stat cache and a work-stealing tree walk; parallel hashing for large files already exists.
- `agit squash` for checkpoint ranges, preserving provenance in `meta`.
- A `benches/` directory with reproducible scripts and a `BENCHMARKS.md` reporting hardware, commands and results, including the same workloads run against git.
- Token-efficient views: `agit diff --compact`, `agit status --short-json`, `agit log --limit --fields`.

### Acceptance criteria

- All Phase 0 crash-safety criteria still hold with the journal and packs enabled. Pulling the power (simulated by SIGKILL plus discarding unsynced page cache in a VM) after an acknowledged commit never loses that commit.
- A repository written by Phase 0 opens and upgrades in place; a Phase 1 repository reports a `format_version` that Phase 0 refuses cleanly.
- `fsck` verifies pack segments, journal, and ref table and detects a single flipped bit in any of them.

### Benchmark targets

| Workload | Target |
| --- | --- |
| Small commits (1 changed file, depth 3), single process, one core | > 10,000 commits/s sustained for 60 s |
| Same workload, 8 concurrent workspaces on one machine | > 20,000 commits/s aggregate |
| `agit status` on 1,000,000 files, warm stat cache | < 500 ms |
| `agit status` on 1,000,000 files, cold cache (all files re-hashed) | within 1.5x of `git status` on the same tree |
| `agit add` of 100,000 new files totalling 1 GiB | within 1.2x of `git add` |
| Median commit latency at 10k commits/s | < 5 ms; p99 < 20 ms |

Targets are measured on a laptop-class machine with a consumer NVMe drive. Server-class numbers are reported separately when available.

---

## Phase 2: Protocol and server

Clone, push, pull, authentication, replication.

### Deliverables

- `crates/agit-protocol`: versioned message schemas, `have`/`want` negotiation, streaming pack transfer over HTTP/2 and gRPC, resumable uploads, idempotency keys.
- ✅ `crates/agit-server` v0 (single node): repositories, SSH-key sign-in with authorized keys and per-repository scopes, scoped short-lived tokens bound to agent identities, ref CAS. 📋 organisations, per-repository Raft-replicated commit log with ref CAS at the log head, local pack tier plus S3-compatible blob tier with cross-repository dedup, sharding by repository.
- CLI: `agit clone`, `agit push`, `agit pull`, `agit fetch`, `agit remote`.
- Server event log exposed over SSE and WebSocket with `seq` resume.
- First draft of `spec/` for the object formats and wire protocol.

### Acceptance criteria

- A three-node Raft deployment survives the loss of any one node with no acknowledged commit lost and no ref regression; a client retrying a push with the same idempotency key after a leader failover gets the same result exactly once.
- Objects received over the wire are re-hashed before acceptance; a corrupted upload is rejected with `hash_mismatch`.
- A token scoped to `refs/heads/feature/*` cannot update `refs/heads/main` (error code `forbidden`, not retryable).
- Clone of a 1 GiB repository is resumable after a dropped connection without re-transferring completed segments.

### Benchmark targets

| Workload | Target |
| --- | --- |
| Push throughput, one hot repository, one shard (3 replicas, same region) | 10,000 to 20,000 commits/s |
| Platform aggregate across 10 shards, synthetic many-repo workload | 100,000 commits/s |
| Commit acknowledgement latency, same region | p50 < 10 ms, p99 < 50 ms |
| Clone of a 1 GiB repository over 1 Gbit/s | > 80% of link bandwidth |

---

## Phase 3: Platform

Pull requests, reviews, policies, the web UI, and the git bridge.

### Deliverables

- Pull requests as structured objects with reviewers, checks and a decision log; structured review comments (`question`, `suggestion`, `blocker`) with resolution; API for agents to open, review and merge. [RFC 0005](rfcs/0005-pull-requests-and-reviews.md).
- Policy engine: ref patterns per principal, required `intent`/`task`, required checks, max files or bytes per commit, required reviewers; evaluated at log-apply and merge time.
- Webhooks with at-least-once delivery and `seq` deduplication.
- Web UI, first milestone: code browsing with syntax highlighting and symbol navigation, history, blame, diffs. Second milestone: PRs and reviews, branches and workspaces, live agent activity, lease board. [RFC 0006](rfcs/0006-web-ui.md).
- `crates/agit-git-bridge`: `agit git import <url>`, `agit git export`, bidirectional sync with a persistent SHA mapping table.
- Server-side compaction of micro-commit runs into human-sized commits with provenance kept in `meta`.

### Acceptance criteria

- Every web UI screen is rendered from public API calls only; an audit script confirms no endpoint is reachable by the UI that is not documented for agents.
- Importing the git repositories of three well-known open source projects and exporting them again yields byte-identical git trees at every commit and preserves authorship and timestamps.
- A PR merged by an agent in violation of a policy is impossible: the merge is refused with a `policy_violation` error that names the rule.
- Time to first byte for a file view and for a PR view is under 200 ms server-side for a repository with 100k commits.

### Benchmark targets

| Workload | Target |
| --- | --- |
| Web UI file view, p50 server render time | < 100 ms |
| Diff view of a 5,000-line change | < 300 ms |
| Git import of a 100k-commit repository | within 2x of `git clone` wall time for the same repository |

---

## Phase 4: Knowledge base and AI integration

Domain knowledge next to the code, and the tooling agents call directly.

### Deliverables

- `note` object kind, knowledge tree on `refs/knowledge/<branch>`, tag rules, validity (`valid_from`, `superseded_by`). [RFC 0003](rfcs/0003-knowledge-base.md).
- `crates/agit-knowledge`: server-side index; query by path, tag set, commit range, full text; summaries first, bodies on demand.
- CLI: `agit know add | query | link | tags`.
- `agit context <path|task>`: relevant files, recent history, open leases, PRs and notes in one call.
- `agit mcp`: MCP server exposing repository operations and knowledge as tools.
- SDKs in `sdk/python` and `sdk/typescript`, typed from the JSON schemas in `spec/`.
- Provenance queries: `agit log --agent <id>`, `--task <id>`, `--run-id <id>`.
- Web UI knowledge browser.

### Acceptance criteria

- Two agents adding notes on different branches merge cleanly; conflicting edits to the same note produce a `content` conflict handled with the same tooling as code.
- `agit know query --path src/billing/x.rs` returns notes tagged by the matching rules, ordered by confidence and recency, with bodies omitted by default.
- Every `agit --json` schema has a published JSON Schema document in `spec/`, and both SDKs are generated from them with no hand-written type definitions.
- The MCP server passes a conformance suite that exercises each tool against a fixture repository.

### Benchmark targets

| Workload | Target |
| --- | --- |
| Knowledge query by path or tag set on 100k notes | p50 < 50 ms server-side |
| `agit context` for a path in a 100k-commit repository | < 500 ms end to end |
| Token size of `agit status --short-json` on a 1,000-file dirty tree | at least 5x smaller than the full `--json` output |

---

## Phase 5: Self-hosting and 1.0

### Deliverables

- The agit repository is migrated from git to agit with the git bridge and managed with agit from then on; the git mirror is kept read-only.
- Format freeze: `AGO`, `AGT`, `AGI`, pack segment, journal, commit and note JSON, and the wire protocol declared 1.0 with a compatibility policy.
- `spec/` complete and normative; a conformance test suite any implementation can run.
- Security review of the token, policy and verification paths; published threat model.

### Acceptance criteria

- One full development cycle (feature, PR, review, merge, release) of agit happens on agit, by both humans and agents, with no fallback to git.
- A repository created by the 1.0 release opens unchanged in every subsequent 1.x release.
- An independent implementation of the object model written against `spec/` alone round-trips the agit repository's own history and produces identical ids.

### Benchmark targets

Re-run the complete Phase 1 to Phase 4 benchmark suite on the 1.0 release and publish results next to the earlier ones so regressions are visible.
