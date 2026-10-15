# 1. Why agit exists

git is the best developer tool of the last twenty years, and agit keeps as much of it as possible. The case for a new system is not that git is bad; it is that git was designed for a human who makes a handful of commits per day, reads prose, and cleans up after herself. Agents are a different kind of user. This chapter lists the concrete failures, the design answers, and what agit does not solve yet.

## The problems, concretely

### 1. Output you have to parse

An agent that runs `git status` gets a paragraph. It writes a regex, the regex breaks on `LANG=de_DE`, on git 2.23 renaming `checkout` to `switch`, on a porcelain change. Exit codes do not say whether retrying makes sense.

```
$ git push
To github.com:org/repo
 ! [rejected]        main -> main (fetch first)
error: failed to push some refs to 'github.com:org/repo'
```

Compare agit, where the same situation is a document with a code and a flag:

```json
{"ok":false,"error":{"code":"ref_conflict","message":"reference refs/heads/scratch changed concurrently (expected None, found Some(ObjectId(3bf5d8…)))","retryable":true,"details":{"name":"refs/heads/scratch","expected":null,"actual":"3bf5d89c19f4…"}}}
```

### 2. Stale lock files

Kill `git commit` at the wrong moment and `.git/index.lock` stays behind. Every later command fails until a human deletes the file. With ten agents in one repository, crashing mid-operation is not an edge case; it happens hourly (timeouts, OOM, cancelled runs).

agit uses advisory `flock` on `.agit/locks/*`. The kernel releases the lock when the process dies, however it dies. There is nothing to clean up.

### 3. Conflicts discovered after the budget is spent

Two agents are told to work in `src/auth`. Both spend twenty minutes and a few dollars of tokens. The merge fails. git has no primitive for "I am working on `src/auth/**` for the next thirty minutes".

```
$ agit lease claim src --ttl 30m --note "add config loading (DEMO-7)" --holder coder-1
lease 3bd2ec0d8a15 granted to coder-1 for src

$ agit lease claim 'src/**' --holder coder-2 --json
{"ok":false,"error":{"code":"lease_conflict","message":"lease conflict: src/** is held by coder-1 (lease 3bd2ec0d8a15)","retryable":false,"details":{"path":"src/**","holder":"coder-1","lease_id":"3bd2ec0d8a15"}}}
```

The second agent learns in milliseconds, before writing a line, and `agit commit` by coder-2 into `src/main.rs` is refused as well (exit code 3) unless `--force` is passed, which is recorded in the event log.

### 4. Last writer wins on refs

`git push --force` and even plain concurrent pushes to a shared branch give you a race. Scripting compare-and-swap in git means `git update-ref` with an old value and hoping nobody uses porcelain. In agit every ref update, local or remote, states what it expects. A concurrent move fails with `ref_conflict`, which is `retryable: true` (exit 75): re-read, reconcile, try again. Several refs can move in one atomic transaction.

### 5. Commit messages as the only metadata

Why was the change made? Which ticket? Which agent, which model, which run, on whose behalf? In git all of that goes into free text or nowhere. agit stores it as fields of the commit object:

```json
{"id":"83e741ad27e1…","message":"Punctuation","intent":"Add exclamation mark","agent":{"id":"coder-1","model":"demo-model","run_id":"run-001"}}
```

`agit log --agent coder-1`, `--task DEMO-7`, `--path src/config.rs` answer provenance questions without an external database, and the server can require `intent` and `agent` on protected branches.

### 6. No cheap snapshots

An agent wants to snapshot before every risky edit so that a failed attempt can be rolled back. In git that means WIP commits that pollute history, or `git stash` gymnastics that do not include untracked files reliably. An agit checkpoint snapshots the whole working tree, tracked and untracked, onto `refs/checkpoints/<workspace>`, never appears in branch history, and costs one tree walk:

```
$ agit checkpoint -m "before rewriting helper" --json
{"ok":true,"result":{"id":"4cc2e92ef489…","tree":"b37c8a4e…","previous":null,"changes_vs_head":2,"created":true}}
$ agit checkpoint restore 4cc2e92ef489…
```

### 7. Polling to notice each other

Agents run `git log` and `git status` in loops to see what others did. agit appends every state change to `.agit/events.jsonl` with a sequence number; `agit events --follow --since 120` tails it, the server exposes the same stream over SSE and webhooks.

### 8. One index per worktree, heavy worktree setup

One-agent-per-worktree is the right model, but `git worktree` is an afterthought. An agit workspace is created with one command, lands on its own branch `ws/<name>`, shares objects, refs, leases and events, and can be removed without touching anyone else's state.

### 9. Knowledge lives outside the repository

Design decisions, gotchas and specs sit in wikis and chat logs; the agent editing the code never sees them. agit keeps notes with typed tags next to the code, maps paths to tags with rules, marks notes stale when the covered paths change, drafts a note from every merged pull request, and answers "what should I read before touching this path" in one call:

```
$ agit know context src/config.rs
context for src/config.rs
suggested tags: area:core, lang:rust

notes (4):
  n_15761d1fc397 Config is read once at startup [kind:fact, area:core]
  n_1f5d3dce7c3b Config::load() reads GREETING from the environment [kind:gotcha, area:core, lang:rust]
  …
recent commits (4): …
leases (0):
open pull requests (0):
agents active here: coder-1, reviewer-1
```

### 10. SHA-1 and slow status

SHA-1 is collision-broken and slow; git's SHA-256 mode is still not the default. `git status` re-stats everything and often re-hashes. agit hashes with BLAKE3 (multi-threaded above 128 KiB), stores file sizes in tree entries so listings never open blobs, and keeps a stat cache plus cached directory tree ids in the index so a commit costs time proportional to the change. First measurements are in [docs/BENCHMARKS.md](../BENCHMARKS.md): `add`+`commit` of one file in a 2,000-file tree takes 12.5 ms against git's 34.6 ms on the same laptop.

## The design answers

| Problem | Answer | Chapter |
| --- | --- | --- |
| Parsing prose | `--json` envelope, stable codes, `retryable`, exit code classes | [6](06-cli-reference.md) |
| Stale locks | advisory `flock`, temp+rename writes, verify on read | [14](14-architecture-internals.md) |
| Late conflicts | leases with TTL, enforced at commit; lease board; `agit_lease_check` in hooks | [5](05-concepts.md#leases), [12](12-multi-agent-workflows.md) |
| Ref races | compare-and-swap, multi-ref transactions, server CAS at the log head | [5](05-concepts.md#refs-and-compare-and-swap) |
| Missing metadata | `intent`, `task`, `agent`, `meta`, signatures; policies that require them | [5](05-concepts.md#commits-and-metadata) |
| Snapshots | checkpoints outside history, `restore`, `reflog` | [5](05-concepts.md#checkpoints) |
| Polling | event log with `seq`, SSE, webhooks | [5](05-concepts.md#events), [8](08-http-api.md) |
| Worktrees | workspaces | [5](05-concepts.md#workspaces) |
| Lost knowledge | notes, typed tags, rules, staleness, drafts, context packs, local offline store | [11](11-knowledge-base.md) |
| Agent review and merge | typed PR comments, reviews, checks, policies, server-side merge | [8](08-http-api.md#pull-request-lifecycle), [12](12-multi-agent-workflows.md) |
| Identity | self-asserted `X-Agit-Agent`, scoped tokens bound to an agent, key registry, audit log | [13](13-server-administration.md) |

## Honest limits today

agit 0.1.0 is a working system, not a finished one. Things you should know before adopting it:

- **Single node.** `agit-server` keeps everything as files under one data directory. There is no replication; back up the directory ([13](13-server-administration.md#backups)). Raft, sharding and blob tiers are 📋 (RFC 0004).
- **Throughput.** The local core manages a few hundred commits per second per process at default durability; the fsync of the ref file dominates. The journal with group commit that lifts this to the 10k/s target is 📋.
- **Knowledge is server-side JSON** (plus a local cache in `.agit/knowledge/`), not objects in the repository. Notes do not branch or merge with code yet; `refs/knowledge/*` is 📋 (RFC 0003). Search is BM25 over tokens: no stemming, no embeddings.
- **One root token per server** with full access, plus scoped tokens (read/write/admin per repository, optionally bound to an agent). No users, no organisations, no OIDC. The acting agent in request bodies (lease `holder`) is still self-asserted.
- **Policies defend against mistakes, not adversaries.** They key on the agent identity, which an unbound token can set freely.
- **Submodules are young.** The format and `submodule add|update|status|remove` work, but switching branches in a repository whose tree contains a submodule entry currently fails, and clones report the submodule path as modified (see the [FAQ](15-faq.md#known-discrepancies-and-rough-edges)).
- **Tags do not travel.** `agit push` pushes a branch; there is no `push --tags`, so tags stay local.
- **Token-efficient views** (`diff --compact`, `status --short-json`, `log --fields`) and per-schema `version` fields are 📋.
- **No TLS, rate limiting or body limits in the server itself.** Put a reverse proxy in front ([13](13-server-administration.md)).
- **Pre-1.0 formats.** `AGO1`, `AGT1`, `AGI2`, `AGP1`/`AGX1` segments and the JSON schemas may change between minor versions until the Phase 5 freeze.

If none of these are blockers, agit already removes most of the friction in the list above, and the rest of this handbook shows how.
