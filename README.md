# agit

**Version control and collaboration for AI agents. Same workflow as git, re-engineered for machines.**

agit is an open-source version control system and platform that keeps the git mental model (init, add, commit, branch, merge, push, pull) and rebuilds everything underneath for a world where the committers are AI agents making thousands of tiny changes in parallel. Every command speaks JSON, every error has a stable code, every commit carries intent and agent identity, and agents coordinate through leases, workspaces, checkpoints and a live event stream instead of discovering conflicts after the fact. The throughput target is 100,000 micro-commits per second platform-wide.

> **Status: Phase 0, local core.** The object model, index, commit, status, diff, branch, merge, leases, events, workspaces, checkpoints and JSON output are implemented in Rust. No server, no web UI yet. First local measurements against git are in [docs/BENCHMARKS.md](docs/BENCHMARKS.md). See [Roadmap](#roadmap).

Legend used throughout this document and the docs:

| Mark | Meaning |
| --- | --- |
| ✅ | built, in `crates/agit-core` and `crates/agit-cli` today |
| 🚧 | in progress on `main` |
| 📋 | planned, designed in `docs/` but not implemented |

---

## Table of contents

- [Why agit](#why-agit)
- [What agit is](#what-agit-is)
- [Quick start](#quick-start)
- [Core concepts](#core-concepts)
- [For AI agents](#for-ai-agents)
- [Platform](#platform)
- [Performance goals](#performance-goals)
- [Repository layout](#repository-layout)
- [Comparison with git](#comparison-with-git)
- [Roadmap](#roadmap)
- [Dogfooding](#dogfooding)
- [Contributing](#contributing)
- [License](#license)

---

## Why agit

git is the best developer tool of the last twenty years. It was designed for humans who type a handful of commits a day, read prose output, and clean up after themselves. Agents are a different kind of user: they make thousands of tiny changes, run in parallel, crash mid-operation, retry blindly, and have to coordinate without a human in the loop. Used that way, git fails in specific, repeatable ways:

- **Unstructured output.** `git status`, `git log`, `git diff` and every error message are prose. Agents parse them with regexes and break on locale, version and porcelain changes. Exit codes do not tell you whether a retry makes sense.
- **Stale `.lock` files.** A crashed process leaves `index.lock` or `refs/heads/main.lock` behind. Every subsequent operation fails until someone deletes the file by hand. With ten agents in one repository this happens hourly.
- **No way to claim work before conflicts happen.** Two agents editing the same module find out at merge time, after both have spent their budget. git has no primitive for "I am working on `src/auth/**` for the next thirty minutes".
- **Slow status on large trees.** Status re-stats and often re-hashes everything. Agents call status far more often than humans do.
- **SHA-1.** Broken for collision resistance, slow by modern standards, and the SHA-256 migration is still not the default.
- **One index per worktree, heavy worktree setup.** One-agent-per-worktree is the right model, but `git worktree` is an afterthought with its own failure modes.
- **Commit message as the only metadata.** Why a change was made, which task it belongs to, which agent and model produced it, who it was done on behalf of: all of that ends up in free text or nowhere.
- **No event stream.** Agents poll `git status` and `git log` in loops to notice each other's work.
- **No cheap checkpoints.** Agents want to snapshot every few seconds so they can roll back a failed attempt. In git that pollutes history or requires stash gymnastics.
- **Knowledge lives outside the repo.** Design decisions, gotchas and specs sit in wikis and chat logs. The agent editing the code never sees them.

agit addresses each of these directly rather than wrapping git.

## What agit is

**The same workflow as git.** `agit init`, `agit add`, `agit commit`, `agit status`, `agit log`, `agit diff`, `agit branch`, `agit switch`, `agit merge`, `agit tag`, and later `clone`, `push`, `pull`. Humans do not need to learn anything new. Default branch is `main`. `.gitignore` is honored alongside `.agitignore`.

**Plus agent-native primitives:**

| Primitive | What it does | Status |
| --- | --- | --- |
| `--json` on every command | Stable, deterministic machine output; errors as `{code, message, retryable}` | ✅ |
| Intent and agent metadata | `--intent`, `--task`, `--agent`, `--model`, `--run-id`, `--meta` stored as structured commit fields | ✅ |
| Compare-and-swap refs | Every ref update states the value it expects; concurrent moves fail with a retryable `ref_conflict` | ✅ |
| Advisory `flock` locks | No stale lock files after a crash | ✅ |
| Leases | Time-limited, glob-based claims on paths; `commit` refuses to write into another holder's exclusive lease | ✅ |
| Workspaces | Many working directories, one object store, one workspace per agent | ✅ |
| Checkpoints | Cheap disposable snapshots on `refs/checkpoints/<workspace>` for rollback, outside branch history | ✅ |
| Event log | Append-only `events.jsonl` with sequence numbers; `agit events --follow` | ✅ |
| BLAKE3 ids, zstd objects, verify-on-read | Fast and safe content addressing | ✅ |
| Pack segments, journal, group commit | Throughput path for micro-commits | 📋 Phase 1 |
| Server, protocol, Raft log, blob tiers | Hosting and replication | 📋 Phase 2 |
| Structured PRs and reviews, policies, web UI, git bridge | The platform | 📋 Phase 3 |
| Knowledge base, context packs, MCP server, SDKs | AI integration | 📋 Phase 4 |
| Self-hosting, 1.0 format freeze, spec | Stability | 📋 Phase 5 |

## Quick start

Requires a Rust toolchain (1.80 or newer).

```sh
git clone https://github.com/agit-vcs/agit
cd agit
cargo build --release
export PATH="$PWD/target/release:$PATH"
```

Create a repository and make a first commit. Everything accepts `--json`; the human output is shown here where it is shorter.

```sh
mkdir demo && cd demo
agit init
echo '# demo' > README.md
agit add README.md
agit commit -m "Add README" \
  --intent "Bootstrap the project" \
  --task DEMO-1 \
  --agent planner-1 --model my-model-v3 --run-id run-0001
```

Ask for status as JSON. Nothing to parse, nothing to guess:

```sh
agit status --json
```

```json
{
  "ok": true,
  "result": {
    "workspace": "main",
    "branch": "main",
    "head": "7f3c0d9e1a4b…",
    "staged": [],
    "unstaged": [],
    "untracked": [],
    "conflicts": []
  }
}
```

Every JSON response is wrapped in the same envelope: `{"ok": true, "result": …}` on success, `{"ok": false, "error": {…}}` on failure. `--json` can also be switched on with `AGIT_JSON=1`, and the agent flags can come from `AGIT_AGENT`, `AGIT_MODEL`, `AGIT_RUN_ID` and `AGIT_ON_BEHALF_OF`, so an orchestrator sets them once per run.

Claim a lease before you start editing. A second agent trying to claim an overlapping exclusive lease gets `lease_conflict`; a commit touching those paths from another holder is refused without `--force`:

```sh
agit lease claim src/auth 'docs/auth/**' --holder coder-2 --ttl 30m --note "refactor login"
agit lease list --json
agit lease check src/auth/login.rs --holder coder-3     # may coder-3 touch this?
agit lease renew <lease-id> --ttl 30m
agit lease release <lease-id>
agit lease release --all --holder coder-2               # agent shutdown
```

Give each agent its own workspace. A workspace is a separate working directory (by default the sibling directory `<repo>.<name>`) on its own branch `ws/<name>`, sharing the object store, refs, leases and events with the primary workspace:

```sh
agit ws new coder-2 --agent coder-2            # creates ../demo.coder-2 on branch ws/coder-2
agit ws new coder-3 --path /work/coder-3 --from main
agit ws list --json
agit ws remove coder-2 --delete
```

Take a checkpoint between real commits. A checkpoint snapshots the whole working tree, tracked and untracked, onto `refs/checkpoints/<workspace>` without moving HEAD or touching the index. It never enters branch history unless you squash it in:

```sh
agit checkpoint -m "before trying the async rewrite"
agit checkpoint list --json
agit checkpoint restore <checkpoint-id>      # roll the working tree back
```

Follow what everyone else is doing without polling:

```sh
agit events --follow
agit events --since 120 --json
agit events --follow --kind lease.           # only lease.claim / lease.renew / lease.release
```

Each line is one event:

```json
{"seq":121,"time_ms":1760480000123,"kind":"lease.claim","workspace":"coder-2","actor":"coder-2","data":{"id":"a1b2c3d4e5f6","paths":["src/auth"],"mode":"exclusive","expires_ms":1760481800123}}
{"seq":122,"time_ms":1760480004417,"kind":"commit","workspace":"coder-2","actor":"coder-2","data":{"id":"9d2e…","branch":"main","intent":"Allow users to sign in with email"}}
```

The rest of the git-shaped surface works as you expect:

```sh
agit branch feature/login
agit switch feature/login
agit diff
agit log --json
agit show HEAD
agit merge main
agit tag v0.1.0
agit cat-file <id>
agit ls-tree HEAD
agit rev-parse HEAD
agit fsck
```

Full command list in Phase 0: `init, add, rm, reset, status, commit, checkpoint (list | restore), restore, log, show, diff, branch, switch` (alias `checkout`), `merge, tag, cat-file, ls-tree, lease (claim | release | list | renew | check), ws (new | list | remove), events, rev-parse, fsck, config`. Every one of them takes `--json` and `-C <dir>`.

## Core concepts

### Objects ✅

agit is content-addressed like git. An object id is the BLAKE3-256 hash of a one-byte kind tag followed by the payload, written as 64 hex characters. The kind tag means a blob can never collide with a tree that happens to contain the same bytes.

- **blob**: raw file bytes.
- **tree**: a binary `AGT1` record list, sorted bytewise by name. Each entry carries mode (`file`, `executable`, `symlink`, `dir`, `submodule`), id, size, and name. The size is stored so `status` and `ls-tree` never need to open blobs.
- **commit**: canonical JSON. Fields: `tree`, `parents`, `author{name,email}`, `time_ms`, `message`, `intent`, `task`, `agent{id,model,run_id,on_behalf_of}`, `checkpoint`, `meta`.

```json
{
  "tree": "4c9a…",
  "parents": ["7f3c…"],
  "author": {"name": "Daniel", "email": "daniel@example.com"},
  "time_ms": 1760480004417,
  "message": "Add login endpoint\n\nUses the existing session store.",
  "intent": "Allow users to sign in with email",
  "task": "PROJ-123",
  "agent": {"id": "coder-2", "model": "my-model-v3", "run_id": "run-0042", "on_behalf_of": "daniel"},
  "meta": {"ci.tests": "passed", "tool.version": "0.1.0"}
}
```

Objects are stored as `objects/xx/yyyy…` files with the header `AGO1 | kind | raw_len | zstd(payload)`, written to a temp file and renamed into place, and re-hashed on every read. A corrupt or tampered object returns `hash_mismatch` or `corrupt_object`; it is never silently served. Exact byte layouts are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

### Refs with compare-and-swap ✅

`refs/heads/*`, `refs/tags/*` and `refs/checkpoints/*` are named pointers to commits. Every update is a compare-and-swap: the caller states what it expects the ref to be (a specific id, absent, or any). If another agent moved the ref in the meantime the update fails with `ref_conflict`, which is marked retryable: re-read, rebase or re-merge, try again. Several refs can be updated in one atomic transaction (all or nothing) for operations that must move more than one ref together.

### Workspaces ✅

A workspace is a working directory with its own `HEAD` and index that shares the repository's object store, refs, leases and event log. `agit ws new <name>` creates one in a sibling directory (or `--path`), starting from HEAD (or `--from <rev>`), on a fresh branch `refs/heads/ws/<name>` so two agents never share a branch head by accident. The secondary working directory holds a small `.agit` pointer file back to the primary repository. The primary workspace is called `main`. The intended model is one workspace per agent: agents never trip over each other's index, and a crashed agent's workspace can be removed without touching anyone else's state.

### Leases ✅

A lease is an advisory, time-limited claim on a set of path patterns, held by a named holder, in `exclusive` or `shared` mode. A literal path covers itself and everything beneath it (`src/auth` covers `src/auth/login.rs`); glob patterns (`src/**/*.rs`) work too. Claiming an exclusive lease that overlaps another holder's active exclusive lease fails with `lease_conflict`. `agit commit` refuses to touch paths under another holder's exclusive lease unless `--force` is passed. Leases carry a TTL and expire on their own, so a crashed agent cannot block work indefinitely. Leases are advisory: nothing stops a raw filesystem write, by design.

### Checkpoints ✅

A checkpoint is a commit with `"checkpoint": true` whose tree is a snapshot of the entire working tree, including untracked files, and whose parent is the current HEAD. It is stored on `refs/checkpoints/<workspace>`; the previous checkpoint is linked through `meta.agit.previous_checkpoint`, so `agit checkpoint list` walks the chain while branch history stays untouched. HEAD and the index do not move. Checkpoints are as cheap as any commit and meant to be taken often: before a risky edit, after each tool call, every N seconds. `agit checkpoint restore <id>` (or `agit restore <rev>`) puts the working tree back. Server-side compaction (📋) will collapse runs of micro-commits into human-sized commits while keeping agent provenance in `meta`.

### Events ✅

Every state change appends one JSON line to `.agit/events.jsonl` with a monotonic `seq`: `repo.init`, `commit`, `checkpoint`, `restore`, `ref.update`, `head.switch`, `merge`, `lease.claim`, `lease.renew`, `lease.release`, `workspace.create`, `workspace.remove`. `agit events --since <seq>` returns everything after a sequence number; `--follow` tails; `--kind <prefix>` filters. On the platform (📋) the same log is exposed over SSE/WebSocket and webhooks.

### Intent and agent metadata ✅

`intent` answers "why", separate from the message's "what". `task` links to a ticket. `agent` identifies who (or what) produced the commit, with model, run id and the principal it acted on behalf of. `meta` is a namespaced JSON map (`tool.name`, `ci.result`) for anything else; `agit commit --meta key=value` sets entries and parses the value as JSON when it can. All of this is in the commit object itself, so it survives clone, push and export, and provenance queries like `agit log --agent coder-2`, `agit log --task PROJ-123` and `agit log --path src/auth` need no external database.

### Knowledge base 📋

Notes (title, markdown body, typed tags, links to paths/symbols/commits/PRs, validity range, author or agent, confidence) stored as versioned objects next to the code and indexed server-side. Tag rules map path patterns to tags so an agent knows what to read before touching a path and what to tag after. Designed in [RFC 0003](docs/rfcs/0003-knowledge-base.md).

### Submodules 📋 (format ✅)

The tree format already has a `submodule` entry mode (`0o160000`) that pins a commit id of a linked repository. `.agitmodules` (TOML) maps path to url, branch and update policy. The `agit submodule add | update | status | foreach` commands and cross-repo PRs are planned; see [RFC 0007](docs/rfcs/0007-submodules.md).

## For AI agents

### The JSON contract ✅

Every command accepts `--json` (or `AGIT_JSON=1`). Output is one JSON document in a fixed envelope, or one JSON object per line for streaming commands like `events --follow`:

```json
{"ok": true, "result": { … command-specific … }}
```

Key order and list order are deterministic. Ids are always full 64-character hex in JSON; short prefixes exist for humans only. A per-schema `version` field inside `result` is planned (📋) so schemas can evolve without breaking you; until then, additions are backwards compatible and removals do not happen without a release note.

Exit codes: `0` success, `1` error, `3` conflict (`merge_conflicts`, `lease_conflict`, `dirty_worktree`), `75` retryable (`ref_conflict`, `lock_timeout`). An agent that only looks at the exit code can still make the right decision.

### Errors and error codes ✅

On failure the process exits non-zero and, with `--json`, prints exactly one object:

```json
{
  "ok": false,
  "error": {
    "code": "ref_conflict",
    "message": "reference refs/heads/main changed concurrently (expected Some(7f3c…), found Some(9d2e…))",
    "retryable": true
  }
}
```

Branch on `code`, never on `message`. Current codes:

| code | meaning | retryable |
| --- | --- | --- |
| `not_a_repository` | no `.agit/` here or in any parent | no |
| `already_exists` | `agit init` in an existing repository | no |
| `object_not_found` | id is not in the store | no |
| `corrupt_object` | object failed to decode | no |
| `hash_mismatch` | object content does not hash to its id | no |
| `ref_not_found` | named ref does not exist | no |
| `invalid_ref_name` | ref name violates the naming rules | no |
| `ref_conflict` | compare-and-swap failed; ref moved | **yes** |
| `invalid_object_id` | not a 64-char hex id | no |
| `ambiguous_revision` | prefix matches several objects | no |
| `unknown_revision` | revision cannot be resolved | no |
| `path_outside_worktree` | path escapes the working tree | no |
| `path_not_tracked` | path is not in the index | no |
| `nothing_to_commit` | index equals HEAD | no |
| `dirty_worktree` | operation needs a clean tree | no |
| `merge_conflicts` | merge stopped with conflicts | no |
| `lease_conflict` | path is under someone else's exclusive lease | no |
| `lease_not_found` | lease id unknown or expired | no |
| `workspace_exists` | workspace name taken | no |
| `workspace_not_found` | no such workspace | no |
| `lock_timeout` | could not acquire an internal lock in time | **yes** |
| `invalid` | invalid argument or state | no |
| `io` | filesystem error (path in message) | no |
| `serde` | serialization error | no |

`retryable: true` means the identical command may succeed if repeated, possibly after re-reading state. `ref_conflict` means your view of the ref is stale: re-read it, reconcile, retry. `lock_timeout` means another process held an internal lock for longer than the timeout (10 s for refs, 5 s for leases and events); back off and retry. `lease_conflict` is deliberately not retryable: wait for the lease to expire, pick other work, or negotiate.

### Token-efficient views 📋

Agents pay per token. Planned views trim output to what a model needs: `agit diff --compact` (hunks without context, paths grouped), `agit status --short-json` (counts and paths only), `agit log --limit N --fields id,intent,agent`.

### Context packs 📋

`agit context <path|task>` returns, in one call, the files relevant to a path or task, their recent history, open leases on them, related PRs, and the knowledge notes whose tags match. One request instead of eight.

### MCP server and SDKs 📋

`agit mcp` will expose repository operations and the knowledge base as MCP tools. Python and TypeScript SDKs (`sdk/`) will wrap the local CLI and the server API behind one typed interface. Until then, shell out to `agit … --json`; the contract is the same one the SDKs will use.

### Idempotency 📋

Mutating server operations will accept an idempotency key so a retried push after a dropped connection applies once. Locally, object writes are already idempotent (same content, same id, no-op) and ref updates are CAS, which together make retry-after-crash safe today.

## Platform

Everything in this section is 📋 planned (Phases 2 and 3) and designed in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) and the RFCs.

**Server (`crates/agit-server`).** Repositories, organisations, permissions, and short-lived scoped tokens per agent. Per-repository append-only commit log replicated with Raft; ref CAS decided at the log head; content-addressed blob store with a local disk tier and an S3-compatible tier, deduplicated across repositories; horizontal sharding by repository. Streaming pack protocol over HTTP/2 and gRPC (`crates/agit-protocol`).

**Pull requests and reviews.** A PR is a structured object: title, intent, task, agent, reviewers, checks, decision log. Review comments are structured too: path, range, kind (`question`, `suggestion`, `blocker`), resolution. Agents open, review and merge PRs through the same API humans use; policies decide which agents may merge what. [RFC 0005](docs/rfcs/0005-pull-requests-and-reviews.md).

**Policies.** Which agent may push to which branch, require `intent`, require passing checks, maximum files per commit, and similar rules, enforced server-side at ref update time.

**Web UI (`web/`).** Clean, GitHub-like, fast. Code browsing with syntax highlighting and symbol navigation, history and blame, diffs, PRs and reviews, branches and workspaces, live agent activity from the event stream, a lease board showing who is working where, and a knowledge browser. TypeScript and React with server-side rendering. The UI uses only the public API that agents use; there are no private endpoints. [RFC 0006](docs/rfcs/0006-web-ui.md).

**Webhooks and event streaming.** The server's event log is exposed over SSE and WebSocket, and pushed to webhooks.

**Git bridge (`crates/agit-git-bridge`).** `agit git import <url>`, `agit git export`, and bidirectional sync with a git-compatible SHA mapping table so teams can adopt agit incrementally.

## Performance goals

The headline target is **100,000 micro-commits per second platform-wide**, with 10k to 20k per second per shard for a hot repository and more than 10k per second on one laptop core for small local commits.

**These are targets, not measurements.** Phase 0 still writes loose objects and one ref file per commit. The first measurements below are from a single laptop and exist to keep us honest, not to make a claim.

First measurements (Phase 0, macOS, Apple Silicon, APFS, 2,000-file tree, CLI against CLI, median of 200 runs):

| Operation | git 2.x | agit 0.1 |
| --- | ---: | ---: |
| `add` + `commit` of one changed file | 34.6 ms | 12.5 ms |
| `status` on a clean tree | 15.3 ms | 11.1 ms |
| `log -n 20` | 12.7 ms | 3.4 ms |
| in-process `add` + `commit` (library, default durability) | n/a | ~7.5 ms (~134/s) |
| in-process `add` + `commit` (library, `AGIT_FSYNC=none`) | n/a | ~2.7 ms (~370/s) |

The gap between the last two rows is one `fsync` of the ref file. That single syscall is what the Phase 1 journal with group commit amortizes over a whole batch. Method and raw numbers: [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

Durability policy ✅: like git, agit does not fsync every loose object by default. Objects are written atomically (temp file + rename) so a crash never leaves a torn object, and the ref update that makes them reachable is always synced. `AGIT_FSYNC=all` syncs everything, `AGIT_FSYNC=none` syncs nothing (throwaway CI repositories).

How the design gets there:

- **BLAKE3.** Several times faster than SHA-1 and SHA-256 on one core, and parallel over large inputs (agit already switches to multi-threaded hashing above 128 KiB). Hashing stops being the bottleneck.
- **Sizes in trees, stat cache in the index.** `status` compares mtime, inode and size before hashing anything, and never opens blobs to answer questions a tree already answers. ✅
- **zstd objects.** Level 3, a good speed/ratio trade-off for source text. ✅
- **No per-object fsync in bulk paths.** 📋 Objects go into append-only pack segments (`packs/*.seg`) with an in-memory and on-disk index; loose objects remain only for tiny repositories.
- **Journal with group commit.** 📋 Micro-commits append to `journal/` and one fsync covers a whole batch. At a 5 ms batching window and 10k commits per second, that is 50 commits per fsync and 200 fsyncs per second, well inside what a single NVMe device sustains.
- **Memory-mapped ref table with CAS.** 📋 Ref updates stop being file renames.
- **Parallel tree walking.** `status` at one million files is a Phase 1 benchmark target.
- **Server: Raft log per repository, batched fsync, blob tiers, sharding by repository.** 📋 Throughput scales with the number of shards; the per-shard figure is bounded by log replication, not by object writes.

The worked estimate for fsync batching and the pack/journal layout are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#planned-pack-and-journal-design) and [RFC 0004](docs/rfcs/0004-throughput.md).

## Repository layout

agit is a monorepo today. Each top-level component is designed to be split into its own repository later and linked back as a submodule through `.agitmodules`.

```
agit/
  Cargo.toml                 Rust workspace
  crates/
    agit-core/        ✅     object model, store, index, refs, diff, merge,
                             leases, events, workspaces, revisions (library)
    agit-cli/         ✅     the `agit` binary (clap), human + --json output
    agit-protocol/    📋     wire protocol, pack streaming, auth
    agit-server/      📋     hosting server: repos, refs, PRs, knowledge, API
    agit-knowledge/   📋     knowledge base: tagged notes linked to code
    agit-git-bridge/  📋     import/export/sync with git repositories
  web/                📋     web UI (GitHub-like, clean): code browsing,
                             history, PRs/reviews, agent activity, knowledge
  sdk/
    python/           📋     agent SDK
    typescript/       📋     agent SDK
  docs/
    ARCHITECTURE.md          deep dive
    ROADMAP.md               phases
    rfcs/                    design documents, one per subsystem
  spec/               📋     on-disk + wire format spec (versioned)
```

Planned sub-repositories: `agit-server`, `agit-protocol`, `agit-knowledge`, `agit-git-bridge`, `web`, `sdk/python`, `sdk/typescript`, `spec`. The core and CLI stay in the main repository. The 📋 directories currently contain only a README describing the component.

## Comparison with git

| | git | agit |
| --- | --- | --- |
| Workflow | init/add/commit/branch/merge/push/pull | same commands, same mental model |
| Hash | SHA-1 (SHA-256 optional) | BLAKE3-256 |
| Object encoding | zlib, text headers | zstd, `AGO1` binary header, kind-tagged ids |
| Verification | on fsck and some reads | every read re-hashes |
| Machine output | porcelain formats, prose errors | `--json` everywhere, stable error codes, `retryable` flag |
| Locking | `.lock` sentinel files | advisory `flock`, no stale locks |
| Ref updates | last writer wins unless you script CAS | compare-and-swap always, multi-ref transactions |
| Work claiming | none | leases with TTL, glob patterns, commit enforcement |
| Multiple working dirs | `git worktree` | workspaces, one per agent, shared store |
| Cheap snapshots | stash, WIP commits | checkpoints on `refs/checkpoints/*`, outside branch history, include untracked files |
| Change notification | poll | `events.jsonl`, `agit events --follow` |
| Commit metadata | message, author, committer | plus intent, task, agent{id, model, run_id, on_behalf_of}, meta |
| Tree entries | mode, id, name | plus size (status without opening blobs) |
| Knowledge | outside the repo | 📋 versioned note objects with typed tags |
| Throughput design | one process, per-object writes | 📋 pack segments, journal + group commit, Raft log, sharding |
| Hosting | GitHub, GitLab, and others | 📋 agit-server with structured PRs, policies, event streaming |
| Maturity | 20 years | Phase 0 |

## Roadmap

| Phase | Theme | Status |
| --- | --- | --- |
| 0 | Local core: objects, index, commit, status, diff, branch, merge, leases, events, workspaces, checkpoints, JSON output, tests | ✅ / 🚧 |
| 1 | Performance: pack segments, journal + group commit, mmap ref table, parallel status at 1M files, published benchmarks vs git | 📋 |
| 2 | Protocol + server: clone/push/pull, auth, Raft log, blob tiers | 📋 |
| 3 | Platform: PRs, reviews, policies, webhooks, event streaming, web UI (code browsing first, then PRs), git bridge | 📋 |
| 4 | Knowledge base + AI integration: notes, tags, query API, context packs, MCP server, SDKs | 📋 |
| 5 | Self-hosting: agit repo managed by agit, 1.0 format freeze, spec | 📋 |

Deliverables, acceptance criteria and benchmark targets per phase are in [docs/ROADMAP.md](docs/ROADMAP.md).

## Dogfooding

This repository is managed with git until agit is self-hosting (Phase 5). The switch is a milestone, not a shortcut: it happens when `clone`, `push`, `pull`, pull requests and the git bridge are solid enough that the agit project itself can live on agit without losing anything. `.agitignore` and `.agitmodules` are already present so the layout does not change on that day.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The short version: `cargo test --workspace`, `cargo clippy -- -D warnings`, format changes need an RFC, every command needs `--json`, no stale locks, no silent data loss. Design discussions happen in `docs/rfcs/`. Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in agit by you shall be dual licensed as above, without any additional terms or conditions.
