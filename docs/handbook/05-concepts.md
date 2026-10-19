# 5. Concepts

This chapter explains every noun in agit: what it is, how it is identified, where it lives, and how it behaves under concurrency. Byte layouts and algorithms are in [14. Architecture and internals](14-architecture-internals.md); the normative description is [docs/ARCHITECTURE.md](../ARCHITECTURE.md) and the RFCs.

## Objects and ids

agit is content-addressed like git. There are three object kinds: **blob** (file bytes, unmodified), **tree** (a directory listing) and **commit**. An object id is

```
id = BLAKE3-256( kind_tag:u8 || payload )      kind_tag: blob = 1, tree = 2, commit = 3
```

rendered as 64 lowercase hex characters. Because the kind tag is inside the hash, a blob and a tree with identical bytes can never share an id. `ObjectId::ZERO` is reserved. Prefixes of at least 4 hex characters resolve when unambiguous (`agit rev-parse 486a50ed`); an ambiguous prefix is `ambiguous_revision`.

Objects are immutable and never rewritten by a newer writer, because re-encoding could change the id. Every read re-hashes the payload and compares it with the requested id; a mismatch is `hash_mismatch` and the data is not returned. `agit fsck` checks every object and every ref.

## Trees

A tree is a binary `AGT1` record list sorted bytewise by name. Each entry carries a mode (`file` 0o100644, `executable` 0o100755, `symlink` 0o120000, `dir` 0o040000, `submodule` 0o160000), a 32-byte id, a 64-bit **size** and the name. The size is the blob length for files, the number of entries for directories and 0 for submodules; it is why `ls-tree`, `status` and the web UI never need to open blobs to answer "how big is this".

```sh
$ agit ls-tree HEAD
100644 2fb2f36a… 60	.agitmodules
040000 c4bd23c8… 2	src
040000 fca4af59… 1	vendor
```

Decoders reject bad magic, unknown modes, unsorted or duplicate names, non-UTF-8 names and trailing bytes as `corrupt_object`.

## Commits and metadata

A commit is canonical JSON (fields in declaration order, maps sorted), so `decode(encode(x)) == x` and the id is stable:

```json
{
  "tree": "560556cb…",
  "parents": ["44b9a0f2…"],
  "author": {"name": "coder-1", "email": "coder-1@agents.example"},
  "time_ms": 1792092751096,
  "message": "Load the greeting from Config",
  "intent": "Make the greeting configurable for DEMO-7",
  "task": "DEMO-7",
  "agent": {"id": "coder-1", "model": "demo-model-1", "run_id": "run-2026-10-15-a"},
  "meta": {"ci.tests": "pending"},
  "signature": {"alg": "ed25519", "signer": "coder-1", "key_id": "0ae983c0881ef763", "public_key": "da2e81ef…", "sig": "…"}
}
```

| Field | Meaning |
| --- | --- |
| `tree`, `parents` | root tree; zero parents for a root commit, two or more for merges |
| `author` | from `agit config user.*`, else `AGIT_AUTHOR_NAME`/`AGIT_AUTHOR_EMAIL`, else the OS user |
| `time_ms` | Unix milliseconds, UTC |
| `message` | first line is the summary |
| `intent` | why the change exists; `--intent` |
| `task` | external ticket; `--task` |
| `agent` | `{id, model?, run_id?, on_behalf_of?}` from `--agent`/`AGIT_AGENT` etc. Omitted when no agent is given |
| `checkpoint` | `true` on checkpoints, omitted otherwise |
| `meta` | namespaced map (`ci.tests`, `git.sha`, `agit.previous_checkpoint`); `--meta k=v`, value parsed as JSON when possible |
| `signature` | optional Ed25519 signature over the canonical bytes without this field |

There is no committer field. Whoever rewrites history keeps the original `author` and records itself in `agent` or `meta`. Optional fields are omitted, never `null`.

Revision syntax understood by the CLI: full or prefixed ids, branch and tag names, `HEAD`, `rev~N`, `rev^N`, and `refs/...` names. `HEAD@{n}` from the reflog display is not accepted. The server's revision parser accepts ids, branch and tag names and `HEAD`, but not `~`/`^` ([FAQ](15-faq.md#known-discrepancies-and-rough-edges)).

## Refs and compare-and-swap

A ref is a named pointer to a commit: `refs/heads/<branch>`, `refs/tags/<tag>`, `refs/checkpoints/<workspace>`, `refs/remotes/<remote>/<branch>`, plus `HEAD` per workspace (symbolic `ref: refs/heads/main` or a detached id). Names must start with `refs/`, must not end in `/` or `.lock`, and must not contain `//`, `..`, control characters or `space ~ ^ : ? * [ \`.

Every update is a **compare-and-swap**: the caller states what it expects the ref to be, `Any`, `Absent` or `Is(id)`. The update happens under one lock only if the expectation holds; otherwise it fails with `ref_conflict {name, expected, actual}`, which is retryable. Commits, merges, branch creation and tags all use `Is(old)` or `Absent`; `Any` is reserved for explicit `--force` paths and checkpoints. Several refs can be updated in one transaction: all expectations are checked first, then all writes happen, so two agents racing for `main` get exactly one success.

```sh
$ curl -s -X POST … /repos/demo/refs/update -d '{"updates":[{"name":"refs/heads/scratch","expect":{"absent":true},"new":"3bf5d89c…"}]}'
{"ok":false,"error":{"code":"ref_conflict","message":"reference refs/heads/scratch changed concurrently (expected None, found Some(ObjectId(3bf5d8…)))","retryable":true,"details":{"name":"refs/heads/scratch","expected":null,"actual":"3bf5d89c…"}}}
```

On the server a push is a CAS against the remote branch; `agit push` sends the objects first, then the ref update with `expect = the last fetched value`. A non-fast-forward push is rejected client-side as `invalid` ("run `agit pull` first or push with --force"); a real race on the server is a `ref_conflict`.

## The index

The index (staging area) is a sorted map of `path → {id, mode, size, mtime_ns, ino, conflict}` plus a cache of directory tree ids. The stat cache lets `status` skip hashing files whose mtime, inode and size are unchanged; the tree cache lets `write_tree` re-encode only directories whose contents changed, so a commit costs time proportional to the change, not the repository. After a `switch` or `merge` the index is rebuilt from a tree with zeroed stat fields, so the next `status` hashes each file once and then warms the cache. Conflicted paths are marked and block `write_tree` until cleared with `agit add`.

The index is a cache: a checksum failure is reported as `invalid`, and it can always be rebuilt from HEAD. Each workspace has its own index.

## Workspaces

A workspace is a pair of `HEAD` and index that shares everything else in `.agit/` (objects, refs, leases, events, reflogs). The primary workspace is called `main` and lives in `.agit/` itself. `agit ws new <name>` creates a secondary workspace:

1. validates the name (no `/`, `\`, `.` or spaces, not `main`);
2. creates the working directory, by default the sibling `<repo>.<name>` (or `--path`), which must be empty;
3. creates the branch `refs/heads/ws/<name>` at the start revision (`--from`, default the current HEAD) with `Expect::Absent`, so two agents never share a branch head by accident;
4. writes `.agit/workspaces/<name>/{HEAD,index,meta.json}` (`meta.json`: name, path, created_ms, agent);
5. writes a `.agit` **file** (not a directory) into the new working directory, `{"common_dir": "...", "workspace": "coder-2"}`, and appends `workspace.create`.

A commit made in one workspace is immediately visible to `log` in another; a lease claimed from one binds commits in all of them. `agit ws remove <name> [--delete]` removes the registration (and the directory); the `ws/<name>` branch stays until deleted. The intended model is one workspace per agent.

## Leases

A lease is an advisory, time-limited claim by a named holder on a set of path patterns, in `exclusive` or `shared` mode:

```json
{"id":"3bd2ec0d8a15","holder":"coder-1","paths":["src"],"mode":"exclusive","created_ms":1792092751063,"expires_ms":1792094551063,"note":"add config loading (DEMO-7)","workspace":"main"}
```

- **Coverage.** A literal pattern covers itself and everything beneath it (`src/auth` covers `src/auth/login.rs`, not `src/authz/x`). A glob (`* ? [ {`) covers a path when it matches the path or any ancestor, so `src/*` covers `src/a/b` for leasing purposes.
- **Overlap** is tested conservatively when a glob is involved (literal prefixes compared); false positives are accepted, false negatives are bugs.
- **Claiming** an exclusive lease fails with `lease_conflict {path, holder, lease_id}` when any active exclusive lease of a *different* holder overlaps any requested path. The same holder may overlap itself. Shared leases never conflict in either direction; they are information for others.
- **Enforcement.** `agit commit` collects the paths that differ from the parent tree and refuses with `lease_conflict` if any is under another holder's exclusive lease; `merge` checks the incoming side the same way. `--force` bypasses the check and is recorded in the event. Nothing intercepts raw file writes: leases gate the act of recording a change.
- **Expiry** is lazy: `expires_ms <= now` means expired, listings filter expired leases out, files are removed on the next claim. No daemon, no event on expiry. The holder is `--holder`/`AGIT_AGENT`, else the configured user. TTL accepts `s`, `m`, `h`, `d` (default `1h`).
- **Scope.** Local leases live in `.agit/leases/` and are shared by all workspaces of a repository. Server leases (`/repos/{repo}/leases`) are a separate, server-wide set for agents that do not share a filesystem.

Leases are a coordination tool, not a security boundary.

## Checkpoints

A checkpoint is a commit with `checkpoint: true` whose tree is a snapshot of the **entire working tree**, tracked and untracked (ignore rules still apply), and whose parent is the current HEAD. It is stored on `refs/checkpoints/<workspace>`; the previous checkpoint is linked through `meta.agit.previous_checkpoint`, which is how `agit checkpoint list` walks newest first. HEAD and the index do not move. Because a checkpoint hangs off HEAD, not off the previous checkpoint, it is never reachable from a branch and `agit log` never shows it; it shares all unchanged blobs and subtrees with the real history.

`agit checkpoint restore <id>` (or `agit restore <rev>`) rewrites the working tree, including untracked files the checkpoint knew about, and appends a `restore` event. `--if-changed` skips the write when the tree equals the last checkpoint or HEAD. Promoting a checkpoint is an ordinary `add` + `commit`. Server-side compaction of micro-commit runs is 📋.

## Reflog

Every ref move and every HEAD move appends one line to `logs/<refname>` (or `logs/HEAD` in the workspace): `<old> <new> <actor> <time_ms>\t<message>`. `agit reflog [name] [-n N]` reads it newest first; `agit reset [--soft|--mixed|--hard] <rev>` is itself logged, so it is undoable with the id from the previous entry; `agit gc` keeps every object a reflog entry references until the entry expires (`--reflog-expire`, default 90 d).

## Signatures and trust

Commits can carry an Ed25519 signature over their canonical bytes (without the `signature` field). Keys are files, `<name>.key` (32-byte secret, hex, mode 0600) and `<name>.pub`, in `~/.agit/keys/` or `$AGIT_KEYS_DIR`. `agit key generate [name]` creates one (default name: `AGIT_KEY`, else `AGIT_AGENT`, else `default`); `agit commit --sign [name]` signs explicitly; with `AGIT_SIGN=auto` (the default) a commit is signed whenever a key with the default name exists; `AGIT_SIGN=0|off` disables, `1|on` forces.

The public key travels inside the commit, so `agit verify <rev>` works offline and anywhere (`status: unsigned | valid | invalid`). *Trust* is separate: a repository on the server keeps a registry of `(agent, key_id, public_key)` (`agit key register`, `POST /keys`), and the server reports `trusted: true` only when the signing key is registered for the signer. The policy `require_signed` makes every commit reaching a protected branch need a valid signature from a registered key. Anyone holding an admin-capable token can add keys, so key trust is only as strong as token custody.

## Policies

A per-repository document on the server, enforced at direct ref updates (`refs/update`, which is what `agit push` does) and at pull request merges:

```json
{"protected_branches":["main","release/*"],"require_pull_request":true,"require_approvals":1,
 "require_intent":true,"require_agent":false,"require_checks":false,"block_on_blockers":true,
 "allowed_merge_agents":[],"allowed_push_agents":[],"max_files_per_commit":0,"require_signed":false}
```

Rules only apply to protected branches (names or globs; `refs/heads/` prefix optional). Protected branches cannot be deleted. With `require_pull_request` they move only through PR merges; `allowed_push_agents`/`allowed_merge_agents` restrict who may push/merge; `require_approvals` counts the latest decision per reviewer and excludes the author; any `request_changes` blocks; unresolved `blocker` comments block when `block_on_blockers`; `require_checks` needs at least one check and all of them `success`; `require_intent`, `require_agent`, `require_signed` and `max_files_per_commit` are checked on every new commit the update introduces (merge commits are exempt from the file limit). Violations are `403 policy_violation`, never retryable. Policies key on the asserted agent identity; they catch mistakes, not impostors ([13](13-server-administration.md)).

## Events

Every state change appends one JSON line to `.agit/events.jsonl` with a monotonic `seq`:

```json
{"seq":27,"time_ms":1792092645310,"kind":"commit","workspace":"main","actor":"coder-1","data":{"id":"83e741ad…","branch":"main","parents":["b906dfda…"],"summary":"Punctuation","intent":"Add exclamation mark","task":null,"files":1}}
```

Local kinds seen in this handbook: `repo.init`, `commit`, `checkpoint`, `restore`, `ref.update`, `head.switch`, `merge`, `lease.claim`, `lease.renew`, `lease.release`, `workspace.create`, `workspace.remove`, `gc`, `fetch`, `push`. `agit events --since <seq>` returns everything after a sequence number, `--follow` tails (200 ms poll), `--kind <prefix>` filters (`lease.`). Appends take `events.lock`, read the last `seq` from the file tail, write with `O_APPEND` and `fdatasync`; a torn trailing line after a crash is skipped.

The server keeps the same log per repository (`<repo>/events.jsonl`) with additional kinds: `repo.create`, `ref.update` (with `data.ref`), `pull.open|comment|review|merge|close`, `note.create|update|delete|validate|draft|accept`, `knowledge.rules`, `policy.update`, `key.add|remove`, `webhook.create|delete`, `lease.*`, and `commit` for each pushed commit. It is exposed by polling (`GET /events?since=&limit=`, which returns the *last* `limit` events), as Server-Sent Events (`GET /events/stream?since=`, replay then live) and through signed webhooks.

## Knowledge model

Knowledge lives on the server as JSON notes plus a local cache; the design for in-repository note objects is 📋 (RFC 0003). The model today:

- **Note**: `{id, title, body (markdown), tags, paths, commits, pulls, author, confidence (0..1, default 1), created_ms, updated_ms, superseded_by?, valid_at?, draft, stale}`. Ids are `n_<hex>`; local-only notes are `n_local_<hex>` until synced.
- **Typed tags** are `namespace:value` strings. Conventional namespaces: `area` (module or domain), `kind` (`decision`, `gotcha`, `spec`, `howto`, `fact`, `rule`, `change`), `lang`, `service`. Unknown namespaces are allowed; the server currently also accepts a tag without a namespace, although the documentation says it should not.
- **Tag rules** map path patterns to tags (`src/** → area:core, lang:rust`). They answer two questions: which tags describe a path (so which notes to read) and which tags a note about a change there should carry (`suggested_tags` in the context call).
- **Staleness**: `valid_at` is the commit the note was written or last validated against (default: HEAD of the default branch at creation). `stale` is computed on every read and is `true` when any path covered by `paths` differs between `valid_at` and HEAD. Notes without `valid_at` or without `paths` are never stale. `POST /notes/{id}/validate` moves `valid_at` forward; `GET /knowledge/stale` lists stale notes with their `changed_paths`.
- **Drafts**: every successful PR merge creates a `draft: true` note (title = PR title, body with intent, task, changed files and reviews, tags = `kind:change` plus the rules' tags, paths = changed directories as `dir/**`, confidence 0.4, `valid_at` = merged commit). Drafts are hidden from listings, context and tag counts until accepted (`accept`, optionally with a patch) or deleted.
- **Search**: `q=` is BM25 over title (weight 3), tags (2), body and paths (1); tokens are lowercase alphanumeric runs, camelCase and snake_case are indexed whole and split, terms of 3+ characters also match as prefixes at half weight. `tags` (all must match), `path` (notes whose `paths` cover it), `commit` and `pull` are post-filters.
- **Context pack**: `GET /context?path=…[&include=files&budget=6000]` returns notes (fresh first, then stale, drafts excluded), `suggested_tags`, `recent_commits`, `leases` overlapping the path, `open_pulls` touching it, `agents_active` (lease holders plus committers in the last 24 h) and, on request, the file plus up to five siblings trimmed to roughly `budget` tokens.
- **Offline store**: `.agit/knowledge/{notes/,rules.json}` holds local-only notes and a cache of the server's notes and rules. `agit know …` uses the server when the remote is reachable and falls back to the local store on transport errors (`source: "local"`); `agit know sync` uploads local-only notes and refreshes the cache; `--local` never touches the server.

[11. Knowledge base](11-knowledge-base.md) covers tagging strategy, templates and querying.

## Submodules

A tree entry with mode `submodule` pins a commit id of another repository; the id is not expected to exist in the containing repository's store and `fsck` does not treat it as missing. `.agitmodules` (TOML, tracked like any file) maps `path` to `url` and optionally `branch`. The working tree holds a full nested repository at that path (`vendor/lib/.agit`), and the outer walker never descends into a nested repository, so it neither pollutes `status` nor gets hashed into the parent. `agit submodule add <url> <path>` clones, registers and stages the pin; `update` clones missing ones and checks out the pinned commits; `status` reports `ok | modified | missing | unpinned` with pinned and actual ids; `agit add <path>` re-pins. Cross-repository ("stacked") pull requests are 📋 (RFC 0007). Known rough edges: see the [FAQ](15-faq.md#known-discrepancies-and-rough-edges).

## Remotes

A remote is a name and a URL of the form `http(s)://host:port/api/v1/repos/<name>`, stored in `.agit/config.json`. `agit remote add|list|remove`, `agit push [remote] [branch] [--force] [--create]`, `agit fetch [remote] [branches…]` into `refs/remotes/<remote>/…`, `agit pull [remote] [--no-ff]` (fetch + merge of the current branch), `agit clone <url> [dest]`. Push negotiates with `POST /objects/missing`, uploads an object stream, then sends one CAS ref update; fetch asks `POST /fetch` with `wants` and `haves` and receives only what it lacks. Authentication is `AGIT_TOKEN` (bearer); identity is `AGIT_AGENT`. A local directory works as a URL for `clone` and `submodule add` (it copies the object store), but not for `fetch`/`pull` afterwards. Tags are fetched but not pushed in 0.1.0.

## Pack segments and durability

Objects are written to an append-only **pack segment** owned by the writing process (`objects/pack/seg-<pid>-<rand>.agp`, record `kind | raw_len | comp_len | id | zstd`) and become visible to other processes only when the segment's index (`.idx`, `AGX1`) is published, which happens atomically on every flush. `Repository` flushes before every ref update and index save, so a ref never points at an unpublished object (the **publish-on-flush rule**). A segment is rolled after 1,024 objects or 256 MiB. Loose `AGO1` objects from earlier versions are still read; `gc` repacks them. `AGIT_PACK=0|off|false` falls back to loose objects.

Durability is a process-wide policy, `AGIT_FSYNC`:

| Value | Behaviour |
| --- | --- |
| `refs` (default) | the ref journal and metadata writes are synced with `fsync(2)`, exactly what git does; object, index and log writes are atomic (append + published index, or temp + rename) but not individually synced. On Linux `fsync(2)` reaches stable storage; on macOS it hands the data to the drive without flushing its cache (git's default there too), so a kernel crash is survived and a power loss may lose the last commits |
| `full` | like `refs`, with a full flush to stable storage for the journal and metadata (`F_FULLFSYNC` on macOS, `fdatasync` elsewhere); power-loss safe, several times slower per commit on Apple hardware |
| `all` | `full`, plus a full flush of every object, index and log write |
| `none` | never fsync; for throwaway repositories and CI |

A crash before a flush leaves garbage at the tail of a segment that nothing references; the next flush rewrites the index from what the writer knows and the bytes are ignored. Ref updates and multi-ref transactions are single records in `refs.journal`, which is synced with group commit (one fsync per batch of concurrent writers) and replayed over the ref files on open, so a crash between the journal sync and a ref-file rename loses nothing.
