# 15. FAQ

Short answers to questions that come up when people and agents start using agit, followed by the list of discrepancies between the documentation and the 0.1.0 binaries that were found while writing this handbook.

## General

**Is agit a git wrapper?** No. It is a separate implementation with its own object store (BLAKE3, zstd, binary trees, JSON commits). It keeps git's commands and mental model, and the bridge (`agit git import|export`) converts history in both directions, but `.agit/` and `.git/` are unrelated.

**Can I use agit on a project that is in git?** Yes: `agit git import <path> --into <dir>` imports the full history; keep exporting with `agit git export` while others stay on git. Trailers `Intent:`, `Task:`, `Agent:` in git commit messages become structured fields. The agit project itself still lives in git until Phase 5.

**Do humans need to learn new commands?** The git ones work (`init`, `add`, `commit`, `status`, `log`, `diff`, `branch`, `switch`/`checkout`, `merge`, `tag`, `stash`, `revert`, `cherry-pick`, `blame`, `grep`, `reflog`, `reset`, `gc`, `push`, `pull`, `clone`). New ones are `lease`, `ws`, `checkpoint`, `events`, `pr`, `know`, `key`, `verify`, `mcp`, `root`.

**Is it production-ready?** It is 0.1.0: the local core is complete and tested, the server is a single node with file-backed storage and a root token plus scoped tokens. Use it where a single server and a daily backup are acceptable, and read [1. Why](01-why.md#honest-limits-today) for the limits.

**Which platforms?** macOS (arm64, x86_64) and Linux (x86_64, aarch64) binaries; Windows x86_64 zip for the CLI (the Claude Code hooks need bash). Rust 1.89 to build.

## Commits and metadata

**Do I have to pass `--intent` every time?** Locally no. Server policies can require it on protected branches, and the Claude Code guard refuses `agit commit` without it. Treat it as the normal case.

**What goes into `message` versus `intent`?** `message` says what changed, `intent` why it was needed. `task` links the ticket. Everything else (`ci.result`, `tool.version`) goes into `--meta key=value`.

**How do I set the author?** `agit config user.name` and `user.email` per repository, or `AGIT_AUTHOR_NAME`/`AGIT_AUTHOR_EMAIL` in the environment. Without either, commits carry the OS user name.

**Why does my commit say `Agent: coder-1`?** Because `AGIT_AGENT` (or `--agent`) was set. Clear it when you commit as a human.

**Can I rewrite history (rebase, amend)?** Not in 0.1.0. `reset --soft` plus a new commit replaces the last commit on an unpushed branch; `revert` undoes a pushed one. Rebase and squash merges are 📋.

**Are commits signed?** Only when a key exists: `agit key generate` (named after `AGIT_AGENT` by default) makes signing automatic under `AGIT_SIGN=auto`. `agit verify` checks a signature offline; the server's key registry decides whether it is *trusted*.

## Coordination

**Do leases stop file writes?** No. They are advisory: `agit commit` and `merge` refuse to record changes under another holder's exclusive lease, and the Claude Code pre-edit hook blocks edits, but a plain `write()` is not intercepted.

**My lease conflict is with myself.** The holder is the exact string in `--holder`/`AGIT_AGENT`. A lease taken under a different id (after renaming the agent) counts as foreign; release it with `agit lease release <id>` or wait for the TTL.

**Is there an event when a lease expires?** No; expiry is lazy. Watch `lease.release` or compute from `expires_ms`.

**Local leases or server leases?** Agents sharing a filesystem (workspaces, Claude Code) use local leases in `.agit/leases/`; agents on different machines use `POST /repos/{repo}/leases`. They are separate sets.

**Can two agents share a working directory?** Do not. Give each a workspace (`agit ws new`) or a clone.

**What happens to checkpoints over time?** `agit gc` expires them after 30 days by default (`--checkpoint-expire`, `0` keeps all). They never appear in `log`.

**Where did my `ref_conflict` come from when I am alone?** Two processes (for example an editor plugin and a script) wrote the same branch concurrently, or a push raced with a merge on the server. It is retryable: re-read and retry.

## Server and platform

**Does the server need a database?** No. Everything is files under `--data-dir`; back up the directory.

**Can I run it without a token?** Yes, and then it is open to anyone who can reach the port (every request acts as root). Only on `127.0.0.1`.

**How do I give an agent limited access?** Create a scoped token with `POST /auth/tokens` (`scopes: [{repo, access}]`, optionally `agent` and `ttl_secs`). Bound tokens act as that agent regardless of the header.

**Why can my scoped token not set the policy?** Policy, keys, webhooks, knowledge rules, repository create/delete and `/auth/*` need `admin` access.

**Why was my PR merge refused?** Read `error.message`: a reviewer requested changes, unresolved blockers, too few approvals, a failing or missing check, an unsigned commit, a missing intent or agent on a commit, or the actor is not in `allowed_merge_agents`.

**Why did `agit push` fail with `invalid` and not `ref_conflict`?** A non-fast-forward push is detected client-side before any network write; the remedy is the same (`agit pull`, then push).

**Are tags pushed?** No. `agit push` pushes one branch; there is no `push --tags` in 0.1.0, so tags stay local. The server's `/tags` is empty unless refs were created through the API.

**How does the web UI authenticate?** Through the token in its settings popover, stored in `localStorage` and sent as a header. There are no cookies.

## Knowledge

**Where are notes stored?** On the server in `<repo>/knowledge/notes/*.json`, cached and, when offline, created in `.agit/knowledge/` (`n_local_*` ids until `agit know sync`).

**Why is my note stale right after a commit that only touched a comment?** Staleness is per covered path, not per claim: any change to a file under `paths` marks the note stale. Narrow `paths` to the file or function the claim is about, and validate after checking.

**What is a draft note?** A note the server generates from every merged PR, hidden until accepted. List with `?drafts=1`, accept with an optional patch, or delete.

**Does search understand plurals or synonyms?** No. BM25 over exact tokens with prefix matching for terms of three or more characters. camelCase and snake_case identifiers are split, so `SessionStore` is found by `session`.

**Can a note cover several repositories?** No; knowledge is per repository.

## Troubleshooting

**`not_a_repository`.** No `.agit` here or above; `agit root` tells you where you are. Secondary workspaces have a `.agit` *file* pointing at the primary.

**`lock_timeout`.** Another process held `refs.lock` (10 s) or `leases.lock`/`events.lock` (5 s). Retry; if it persists, something is stuck holding a file descriptor, not a stale file.

**`hash_mismatch` or `corrupt_object`.** Storage returned bytes that do not match the id. Run `agit fsck`, restore the object from a clone or backup (`agit fetch` brings it back if the server has it).

**`io … Operation not permitted` on `agit switch`.** The tree contains a submodule entry; see the first item below.

**The Claude Code hooks do nothing.** Check `/hooks` in Claude Code, run `echo '{"cwd":"'"$PWD"'"}' | .claude/hooks/agit/session-start.sh`, make sure `agit` is on the `PATH` Claude Code uses (or set `AGIT_BIN`), and that `.agit` exists ([docs/CLAUDE-CODE.md](../CLAUDE-CODE.md#troubleshooting)).

## Known discrepancies and rough edges

Found while verifying this handbook against agit 0.1.0 (workspace `b1fc6ff`, binaries of 2026-10-15 21:37). None of these are fixed by the handbook; they are listed so you are not surprised, and so they can be tracked.

**Behaviour**

1. **`switch` fails in a repository whose tree contains a submodule entry.** `agit switch <branch>` (also with `--force`) returns `io error at …/vendor/lib: Operation not permitted` whenever the target tree has a `submodule` entry, both in the primary repository (nested repo checked out) and in clones (empty directory). `switch -c <new>` at the current commit still works because no tree change is applied. Cause (from `worktree.rs`): the checkout path treats the submodule entry as changed (the index records size 64 for a submodule, the tree stores 0) and `write_entry` calls `remove_file` on the directory.
2. **Clones of a repository with a submodule report the submodule path as modified.** `agit status --short` shows `M vendor/lib` (and ` D vendor/lib` before `submodule update`), and `agit pull` refuses with `dirty_worktree: ["vendor/lib"]` even after `agit submodule update` reports `ok`. Workaround today: avoid submodules in repositories agents pull into, or use a fresh clone to see new commits.
3. **`fetch`/`pull` inside a submodule cloned from a local path fail** with `not an agit repository url (expected …/api/v1/repos/<name>)`, although `clone` and `submodule add` accept local directories (README: "local directory paths work as URLs too").
4. **`agit merge --json` prints two JSON documents on conflicts**: the `{"ok":true,"result":{"kind":"conflicts",…}}` outcome and then the `merge_conflicts` error envelope (README: "prints exactly one object"). Parse the first for details, the second for the code. `agit verify` on an unsigned commit does the same (result, then an `error` envelope).
5. **`AGIT_JSON=1` is rejected** ("invalid value '1' for '--json'"); only `AGIT_JSON=true` works (README says `AGIT_JSON=1`).
6. **`HEAD@{n}` is displayed by `agit reflog` but not accepted as a revision** (`unknown_revision`); undo with the id.
7. **Reflog actor is not `user.name`**: it is `AGIT_AGENT`, else `AGIT_AUTHOR_NAME`, else `$USER`.
8. **Non-fast-forward pushes are `invalid` (exit 1), not `ref_conflict` (exit 75)**; AGENT-GUIDE.md describes the pull-and-retry remedy under `ref_conflict`. Only a genuine server-side race returns `ref_conflict`.
9. **Tags are not pushed**; `agit fetch` documents "branches and tags" but there is no way to publish a tag from the CLI.
10. **CLI-layer errors have `code: "error"`** (unknown config key, `nothing specified; use agit add`, `merge needs a revision`, invalid duration units, non-core failures), contradicting "every error has a stable code".
11. **`agit know` output shapes differ per subcommand**: `add` returns `{note, source}`, `search` `{notes, source}`, `context` `{context, source}`, while `get` and `tags` return the bare object or list. Clients written against the pre-local-store shape (bare note/list/context) break; this changed with the local knowledge store commit.
12. **The server accepts tags without a namespace** (`--tag nonamespace` succeeds) although API.md and RFC 0003 say typed tags are validated and a tag without a namespace is rejected.
13. **`/api/v1/health` is served without the bearer token**, while DEPLOYMENT.md and SECURITY.md state it sits behind the same check (the Compose health check still sends the token, which is harmless).
14. **Bound tokens do not rewrite request bodies**: a token bound to `coder-1` can `POST /leases` with `"holder":"someone-else"` and the lease is recorded for `someone-else`; only the `X-Agit-Agent` header is overridden.
15. **`GET /refs?prefix=refs/heads/main` returns an `io … Not a directory` error** (HTTP 500) when the prefix names a single ref; use a directory prefix and filter.
16. **Server revision syntax is narrower than the CLI's**: `main~1`, `HEAD^` are `unknown_revision` on `GET /diff`, `/commits/{rev}` etc.; branch names with `/` must be URL-encoded in path segments (`commits/feature%2Fenv`); API.md says `commit=` accepts "any revision syntax".
17. **`policy_violation` messages from merges are double-prefixed** ("policy violation: policy violation: …").
18. **`stash pop`/`apply` report `files_changed: 0`** even when files were restored; `checkpoint --if-changed` with no change reports `previous` equal to the returned id. Cosmetic.
19. **Malformed JSON bodies get a `422` plain-text message** from the framework instead of the error envelope.

**Documentation staleness**

20. README says Rust "1.80 or newer"; `Cargo.toml` and DEPLOYMENT.md say 1.89.
21. README and CLAUDE-CODE.md say the MCP server has 32 tools; the binary lists 33 (`agit_know_sync` added).
22. ARCHITECTURE.md's layered view and storage layout still mark the web UI, SDKs, MCP, pack segments (`packs/*.seg`) and knowledge as 📋, and describe the index as `AGI1` and the store as loose objects; the binaries write `AGI2`, `objects/pack/seg-*.agp` with `AGX1` indexes, and `logs/` reflogs, none of which appear in the layout listing. CLAUDE.md also lists `AGI1`.
23. ROADMAP.md's phase table still shows Phase 2 to 5 as 📋 and the status note above it is the only place that reflects the shipped server, SDKs and MCP.
24. The README's "Repository layout" marks `agit-protocol`, `agit-server`, `agit-git-bridge`, `web/` and both SDKs as 📋 "containing only a README"; they are built.
25. RFC 0003 says tag rules are `rules.toml` and notes live on `refs/knowledge/*`; the implementation note at its top correctly says JSON files and `rules.json`, but the "CLI" section still lists `agit know query|link|tags [--path]`, which do not exist (`search`, `get`, `context`, `tags`, `sync` do).
26. API.md documents the knowledge query param `commit=<rev>` as "any revision syntax" (see 16) and does not yet mention the local store, `n_local_` ids or `agit know sync`.
27. The GitHub organisation changed from `agit-vcs` to `shadow-kernel` in the last commits; the Homebrew formula and `homebrew/agit.rb` comments still say `agit-vcs`.
28. web/README.md says blame has "no server blame endpoint"; `GET /blame/{rev}` exists and is documented in API.md (the UI still computes blame client-side).
