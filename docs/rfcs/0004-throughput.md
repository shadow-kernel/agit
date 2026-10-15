# RFC 0004: Throughput: pack segments, journal, group commit, replicated log

- Status: **Draft**
- Phase: 1 (local) and 2 (server)
- Depends on: RFC 0001

## Summary

Reach more than 10,000 small commits per second on one laptop core locally, 10k to 20k per second per shard on the server for a hot repository, and 100,000 per second platform-wide, without weakening durability. Locally: append-only pack segments, a journal with group commit, and a memory-mapped ref table. On the server: a per-repository Raft-replicated commit log with the ref CAS decided at the log head, tiered blob storage, and sharding by repository.

All figures in this RFC are design targets. Nothing has been measured yet.

## Motivation

Agents commit far more often than humans: a checkpoint after every tool call, a commit per completed step, thousands of agents per platform. Phase 0 writes each object as its own file with its own `fdatasync`, which caps throughput at the device's fsync rate (hundreds per second), not at CPU. The fix is the standard database answer: batch durable writes, append rather than create, keep hot metadata in memory with a durable log behind it.

## Design

### Local

**Pack segments.** `packs/<seg>.seg` is an append-only sequence of records `kind:u8 | raw_len:u64le | zstd frame`, identical to the loose object body after the magic. A sidecar `.idx` (sorted ids → offsets) is written when the segment is sealed at a size threshold; the active segment's index lives in memory and in the journal. Lookup order: memory, sealed indexes, loose objects. `agit gc` repacks loose objects and merges small segments. Loose objects remain the default below a configurable object count so tiny repositories stay trivially inspectable.

**Journal.** `journal/<n>.jnl` holds records of the form:

```
record = len:u32le | crc32c:u32le | body
body   = objects[] (inline bytes or segment offsets) | ref_cas[] (name, expected, new) | event
```

One commit, one multi-ref transaction, one lease change: each is one record. A record is applied atomically or not at all (the CRC rejects torn tails on replay). Startup replays unsealed journals into the in-memory ref map and segment indexes.

**Group commit.** A single writer thread drains a queue of records into the active journal file, issues one `fdatasync` for the batch, and then acknowledges every caller in the batch. The batching window is adaptive: it closes when the queue is empty, when it reaches a byte budget, or at a maximum delay (default 5 ms). Under low load a commit costs one fsync; under high load the fsync is amortized across the batch.

**Ref table.** Refs live in an in-memory map guarded by a mutex; CAS is a compare under the mutex followed by enqueueing the journal record. Durability comes from the journal. A `refs.table` snapshot (sorted array plus string arena, memory-mapped) is rewritten atomically every N batches for fast cold start and for external readers.

**Worked estimate** (laptop NVMe, fsync about 1 ms; small commit about 2 KiB compressed plus 200 bytes record overhead; BLAKE3 about 1 GiB/s per core; zstd level 3 about 400 MiB/s per core):

| quantity | value at 10k commits/s |
| --- | --- |
| commits per 5 ms batch | 50 |
| fsyncs per second | 200 |
| sequential append bandwidth | about 22 MiB/s |
| hash + compress CPU per commit | about 7 µs |
| total CPU budget per commit (encode, tree rebuild, journal) | 30 to 50 µs |
| implied single-core ceiling | about 20k to 30k commits/s |
| median / tail latency | about 3.5 ms / about 6 ms |

The ceiling is CPU, not the device; the design leaves room above the 10k target.

### Server

**Replicated commit log.** Each repository has a Raft group (3 or 5 replicas). A push is a log entry: `ref_cas[]` plus the ids of new objects. The leader appends entries in batches (one fsync per batch per replica), replicates, and applies in order. The CAS is evaluated at apply time against the ref state at the log head, so concurrent pushes to the same ref produce exactly one success and `ref_conflict` for the rest. Followers serve reads.

**Objects first.** Clients stream objects before submitting the log entry; the server re-hashes and stores them in the shard's hot tier and asynchronously in the cold tier. An entry that references an object the server does not have is rejected before entering the log.

**Blob tiers.** Hot: local NVMe pack segments on the shard. Cold: S3-compatible object storage, which is the durability anchor; a shard can be rebuilt by replaying its logs and fetching from cold. Objects are deduplicated across repositories by id.

**Sharding.** A repository is assigned to one shard; a gateway routes by repository id. Aggregate throughput scales with shard count. The target of 100k commits/s platform-wide assumes on the order of 5 to 10 active shards at 10k to 20k each, plus headroom for cold repositories.

**Protocol.** HTTP/2 and gRPC; `have`/`want` negotiation; objects streamed in segment record format so the server can append without re-encoding; resumable uploads; idempotency keys stored with the log entry so a retried push returns the original result.

**Compaction.** Runs of checkpoints and micro-commits can be collapsed server-side into one commit per policy (per task, per PR), with the collapsed chain summarized under `meta.agit.squashed` so provenance survives.

### Benchmarks to publish

Phase 1: single-core commit throughput and latency distribution; 8-workspace aggregate; `status` at 1M files warm and cold; `add` of 1 GiB; all against git on the same machine. Phase 2: per-shard push throughput with 3 replicas, aggregate across 10 shards, acknowledgement latency, clone bandwidth. Scripts and raw data go into `benches/`.

## Alternatives

- **An embedded key-value store (RocksDB, sled) for objects.** Rejected: it hides the on-disk format behind a dependency, makes `spec/` impossible to write without that dependency, and offers little over append-only segments for immutable content.
- **Per-object fsync with many threads.** Rejected: throughput is bounded by the device's fsync rate; batching is the only way past it.
- **Multi-leader or CRDT refs on the server.** Rejected: refs need a total order for CAS semantics; Raft per repository gives that with well-understood failure behavior.
- **Sharding by object rather than by repository.** Rejected for the commit log: cross-shard transactions would be needed for every push. Objects can still be stored in a shared blob tier.

## Open questions

- Adaptive batching policy under mixed workloads (one chatty agent and many idle ones).
- Whether the journal should be per workspace to reduce contention on one machine, merged by a single durability thread.
- Segment sealing threshold and `gc` policy for very large binary blobs (likely a separate "large object" segment class).
- Raft library choice and whether multi-Raft (many groups sharing one log file per shard) is needed at the target shard sizes.
- How the hot tier evicts to cold without hurting clone latency.
