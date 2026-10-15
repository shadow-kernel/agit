# 0. Introduction

agit is a version control system and hosting platform built for a world in which most commits are made by AI agents. It keeps git's mental model and command vocabulary (`init`, `add`, `commit`, `branch`, `merge`, `push`, `pull`, `clone`) and replaces the machinery underneath with one designed for machines that work in parallel, crash, retry and have to coordinate without a human watching.

This handbook documents agit 0.1.0 as built on 2026-10-15 (workspace commit `b1fc6ff`, binaries `target/release/agit` and `target/release/agit-server`). Every command, JSON document and HTTP response shown here was produced by running those binaries in a throwaway repository and against a local `agit-server`; outputs are trimmed, never invented. Where the documentation in `docs/` and the behaviour of the binaries disagree, the handbook follows the binaries and says so (see [15. FAQ](15-faq.md#known-discrepancies-and-rough-edges)).

## Who this is for

| Reader | Start with |
| --- | --- |
| A developer who knows git and wants to use agit day to day | [3. Quick start for developers](03-quickstart-developers.md), then [6. CLI reference](06-cli-reference.md) |
| Someone writing or operating an AI agent (any model, any framework) | [4. Quick start for agents](04-quickstart-agents.md), [7. MCP reference](07-mcp-reference.md), [9. SDKs](09-sdks.md), [12. Multi-agent workflows](12-multi-agent-workflows.md) |
| An operator hosting an `agit-server` for a team | [13. Server administration](13-server-administration.md), [8. HTTP API](08-http-api.md), [10. Web UI](10-web-ui.md) |
| A contributor who wants to understand the formats and algorithms | [5. Concepts](05-concepts.md), [14. Architecture and internals](14-architecture-internals.md) |

A suggested reading order per audience is in the [handbook index](README.md).

## What agit is, in one page

**A local VCS.** `agit init` creates `.agit/`. Files are hashed with BLAKE3, stored zstd-compressed in append-only pack segments, and referenced by trees and commits exactly as in git. `status`, `diff`, `log`, `blame`, `stash`, `revert`, `cherry-pick`, `reflog`, `reset`, `gc`, `grep` and submodules behave the way a git user expects. `.gitignore` is honoured next to `.agitignore`.

**A machine contract.** Every command takes `--json` and prints one envelope, `{"ok": true, "result": ...}` or `{"ok": false, "error": {"code", "message", "retryable", "details"}}`. Exit codes are 0 (success), 1 (error), 3 (conflict: merge, lease or dirty worktree) and 75 (retryable: ref conflict or lock timeout). An agent branches on `error.code`, never on prose.

**Coordination primitives git lacks.**

- *Compare-and-swap refs*: every ref update states the value it expects; a concurrent move fails with a retryable `ref_conflict`. Several refs move atomically.
- *Leases*: time-limited, glob-based claims on paths. `agit commit` refuses to write into another agent's exclusive lease unless forced.
- *Workspaces*: many working directories sharing one object store, one per agent, each on its own branch `ws/<name>`.
- *Checkpoints*: snapshots of the whole working tree (including untracked files) on `refs/checkpoints/<workspace>`, outside branch history, cheap enough to take after every tool call.
- *Events*: an append-only `events.jsonl` with sequence numbers; `agit events --follow` tails it, the server streams it over SSE and webhooks.
- *Intent and provenance*: commits carry `intent`, `task`, `agent{id, model, run_id, on_behalf_of}` and a free `meta` map as structured fields, queryable with `agit log --agent`, `--task`, `--path`.
- *Signatures*: Ed25519 signatures travel inside the commit; a per-repository key registry decides which keys are trusted for which agent.
- *Advisory `flock` locks*: no `.lock` files to clean up after a crash.

**A platform.** `agit-server` hosts repositories over HTTP (`/api/v1`): clone, push and pull with CAS refs, pull requests with typed comments (`question`, `suggestion`, `blocker`, `note`), reviews, checks and server-side merge, per-repository policies (protected branches, PR-only, approvals, required intent/agent/signature, max files per commit), server-wide leases, an SSE event stream, signed webhooks, a root token plus scoped per-agent tokens with an audit log, and a knowledge base: notes with typed tags, tag rules, BM25 search, staleness tracking, draft notes generated from merges, and a one-call `context` endpoint that returns everything an agent should read before touching a path. A React web UI, a Python SDK, a TypeScript SDK, an MCP server (`agit mcp`, 33 tools) and a Claude Code integration (hooks plus MCP) sit on top of the same API.

**Honest status.** agit is pre-1.0. The server is a single node with file-backed storage; replication (Raft), blob tiers, the journal with group commit, in-repository note objects, semantic search and token-efficient output views are designed but not built. The project tracks this in [docs/ROADMAP.md](../ROADMAP.md).

## How agit differs from git, briefly

| Topic | git | agit |
| --- | --- | --- |
| Output | prose, porcelain formats | `--json` everywhere, stable error codes, `retryable` flag, exit code classes |
| Hash and objects | SHA-1, zlib, text headers | BLAKE3-256 with a kind tag inside the hash, zstd, binary trees with sizes, JSON commits, verify on every read |
| Locks | `index.lock`, `refs/heads/x.lock` sentinel files | advisory `flock`; a crashed process leaves nothing behind |
| Ref updates | last writer wins unless scripted | compare-and-swap, multi-ref transactions |
| Claiming work | none | leases with TTL and globs, enforced at commit time |
| Multiple working dirs | `git worktree` | workspaces on their own `ws/<name>` branch, shared store, leases and events |
| Snapshots | stash, WIP commits | checkpoints outside branch history, untracked files included |
| Commit metadata | message, author, committer | plus intent, task, agent identity, meta, optional Ed25519 signature |
| Change notification | polling | event log with `seq`, `--follow`, SSE, webhooks |
| Knowledge | wiki, chat | notes with typed tags and staleness next to the code, a per-path context pack |
| Hosting | GitHub, GitLab and others | `agit-server`: structured PRs, policies, tokens, audit log, same API for humans and agents |

The full table and a discussion of when not to use agit are in [17. Comparison](17-comparison.md).

## Status legend

The handbook uses the same marks as the rest of the project:

| Mark | Meaning |
| --- | --- |
| ✅ | built and verified in this handbook with the 0.1.0 binaries |
| 🚧 | present on `main` in part, or present in source but not yet in the binaries used here |
| 📋 | designed in `docs/` or an RFC, not implemented |

Anything without a mark is ✅.

## Conventions

- Shell examples are `sh`/`zsh` compatible. Lines starting with `$` are commands; everything else is output. Long ids are shortened to 12 characters in prose but are always 64 hex characters in JSON.
- `AGIT_AGENT=coder-1 agit ...` shows the agent identity explicitly. In real deployments an orchestrator sets `AGIT_AGENT`, `AGIT_MODEL` and `AGIT_RUN_ID` once per run.
- Server examples use `http://127.0.0.1:17979/api/v1` and the bearer token `handbook-demo-token`. Replace both.
- Named cross-references point to chapters of this handbook; `docs/...` links point to the primary documents in the repository, which remain the normative source for the HTTP contract ([docs/API.md](../API.md)) and the on-disk formats ([docs/ARCHITECTURE.md](../ARCHITECTURE.md)).
