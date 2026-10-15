# 17. Comparison: agit, git, and GitHub-style platforms

A frank comparison for people deciding whether to adopt agit. "git" means the git CLI; "platform" means GitHub, GitLab, Gitea and similar hosting with pull requests. ✅ built in agit 0.1.0, 📋 designed but not built.

## Local version control

| Capability | git | agit |
| --- | --- | --- |
| Workflow and commands | init/add/commit/branch/merge/push/pull | the same vocabulary, plus `lease`, `ws`, `checkpoint`, `events`, `know`, `key` |
| Hash | SHA-1 (SHA-256 optional, not default) | BLAKE3-256, kind tag inside the hash ✅ |
| Object encoding | zlib, text headers | zstd level 3, binary trees with sizes, JSON commits ✅ |
| Verification | on fsck and some reads | every read re-hashes; index checksummed ✅ |
| Storage | loose objects + packfiles with delta compression | append-only pack segments, no delta compression yet ✅; journal + group commit 📋 |
| Machine output | porcelain formats, prose errors, ambiguous exit codes | `--json` everywhere, stable `error.code`, `retryable`, exit classes 0/1/3/75 ✅ |
| Locking | `.lock` sentinel files that outlive crashes | advisory `flock`, nothing to clean up ✅ |
| Ref updates | last writer wins unless scripted with `update-ref` | compare-and-swap always, multi-ref transactions ✅ |
| Work claiming | none | leases with TTL, globs, commit-time enforcement ✅ |
| Multiple working directories | `git worktree` | workspaces on `ws/<name>`, shared store, leases and events ✅ |
| Cheap snapshots | stash, WIP commits | checkpoints outside history, untracked files included ✅ |
| Undo | reflog, reset | reflog, reset, checkpoints, `restore` ✅ |
| Commit metadata | message, author, committer, GPG/SSH signature | plus intent, task, agent{id, model, run_id, on_behalf_of}, meta; Ed25519 signature with the key inside the commit ✅ |
| Change notification | none (poll) | `events.jsonl` with `seq`, `--follow` ✅ |
| History tools | log, blame, bisect, rebase, filter-branch, submodules, LFS, hooks | log (with agent/task/path filters, graph), blame (first-parent), revert, cherry-pick, stash, grep, submodules ✅; rebase, bisect, LFS, client hooks 📋 or absent |
| Ignore rules | `.gitignore` | `.agitignore` and `.gitignore` ✅ |
| Interop | universal | `agit git import|export` through the git CLI ✅; serving the git protocol 📋 |
| Maturity | 20 years, every tool knows it | 0.1.0, pre-format-freeze |
| Measured speed (2,000-file tree, laptop) | add+commit 34.6 ms, status 15.3 ms, log -n 20 12.7 ms | 12.5 ms, 11.1 ms, 3.4 ms ✅ (BENCHMARKS.md) |

## Hosting and collaboration

| Capability | GitHub-style platform | agit-server |
| --- | --- | --- |
| Deployment | SaaS or a large self-hosted stack (database, cache, workers) | one static binary, one data directory, reverse proxy for TLS ✅ |
| Pull requests | title, markdown body, free-text comments, review states, checks | typed object with intent and task, comments with `kind` (`question`, `suggestion`, `blocker`, `note`) and resolution, reviews, checks, mergeability with conflict preview, server-side merge with CAS ✅; squash/rebase, applyable suggestions, decision log 📋 |
| Branch protection | required reviews, status checks, signed commits, linear history, CODEOWNERS | protected branches (globs), PR-only, approvals (author excluded), block on `request_changes`/blockers, required checks, required intent/agent/signature, allowed push/merge agents, max files per commit ✅; CODEOWNERS 📋 |
| Identity and access | users, orgs, teams, OIDC, fine-grained tokens | root token plus scoped tokens (read/write/admin per repository, bound agent id, TTL), audit log ✅; users, orgs, OIDC 📋 |
| Agent identity | a bot user | first-class `agent{id, model, run_id}` on commits, PRs, reviews and notes; `/agents` activity; bound tokens ✅ |
| Coordination between contributors | none beyond PR status | server-wide leases with TTL, lease board, context endpoint listing active agents ✅ |
| Events | webhooks, GraphQL subscriptions | per-repository event log with `seq`, polling, SSE, signed webhooks ✅ |
| Knowledge | wiki, discussions, issues (separate from code) | notes with typed tags linked to paths/commits/PRs, tag rules, BM25 search, staleness, drafts from merges, one-call context pack, offline store ✅; in-repository note objects, embeddings 📋 |
| Code browsing UI | mature | tree/blob with highlighting, commits with agent/intent/task filters, diffs, compare, client-side blame and search, PRs, agents, leases, knowledge, settings ✅; SSR, symbol navigation 📋 |
| API | REST + GraphQL, huge surface | small REST surface (`docs/API.md`) that is exactly what the UI uses; SDKs for Python and TypeScript; MCP server ✅ |
| CI | integrated runners | none; a CI agent reports checks through the API ✅ |
| Issues, projects, discussions, packages, pages | yes | no (`task` links to your tracker) |
| Replication and scale | managed | single node; Raft, sharding, blob tiers 📋 |
| Git clients | native | only through the bridge; no `git clone` against agit-server 📋 |

## Where agit is better today

- Agents that must act without a human in the loop: structured errors, CAS refs, leases, checkpoints and events remove the guessing and the cleanup.
- Many small commits by many parallel workers in one repository on one machine (workspaces plus local leases).
- Review and merge automation: typed comments, policies that say exactly why a merge was refused, server-side merge with CAS.
- Provenance: `who (agent, model, run) did what, why, for which ticket` is in the commit, queryable without an external system.
- Knowledge delivery: the right notes arrive with the path, stale notes are visible, merges produce drafts.
- Operations: one binary, one directory to back up, an audit log.

## Where git and the platforms are better today

- Ecosystem: every editor, CI system and hosting product speaks git; agit has a bridge, an MCP server, two SDKs and one web UI.
- History surgery: rebase, interactive rebase, bisect, filter-repo, amend.
- Large binaries and huge repositories: delta compression, LFS, partial clone, sparse checkout, commit-graph.
- Hosting features beyond code: issues, projects, discussions, packages, runners, pages, marketplace.
- Scale and availability: replicated, multi-region hosting versus a single node.
- Identity: users, organisations, SSO, fine-grained permissions.
- Stability: formats and APIs that do not change.

## When not to use agit

- You need other people's git clients and tools to work against your repository directly, and the import/export cadence is not acceptable.
- Your repository is large in binaries or in file count beyond what a laptop handles comfortably without delta compression and partial clone.
- You need rebase-based workflows, bisect or history rewriting.
- You need multi-region availability or more than one server per trust domain with shared identity.
- You cannot run a daily backup of a directory and accept a single node.
- You need the formats to be frozen (before Phase 5, minor versions may change them).

## When to use agit

- A team runs several coding agents against the same codebase and keeps hitting late merge conflicts, stale lock files, or "who changed this and why".
- An orchestrator needs machine-readable, retry-safe version control operations and a live event stream.
- You want reviews and merges performed by agents under explicit policies with an audit trail.
- Your project's decisions and gotchas keep getting lost between runs, and you want them delivered with the code.
- You can keep a git mirror through the bridge for the humans and tools that need git, while the agents work in agit.

The project's own plan is to move its development onto agit once the bridge and the server are solid (Phase 5, [docs/ROADMAP.md](../ROADMAP.md)).
