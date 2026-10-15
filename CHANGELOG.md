# Changelog

All notable changes to agit are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
uses [Semantic Versioning](https://semver.org/). Until 1.0, minor versions
may change on-disk and wire formats; each such change is called out under
"Changed" with the affected format version (`AGO1`, `AGT1`, `AGI1`, commit
JSON, API `v1`).

Versions are cut with `scripts/release.sh <version>`, which moves the
Unreleased section below into a dated release and tags it; the first tagged
release will be 0.1.0 (the current workspace version).

## [Unreleased]

### Added

Local core (`agit`, `crates/agit-core`, `crates/agit-cli`):

- git's workflow: `init`, `add`, `commit`, `status`, `log`, `diff`, `branch`,
  `switch`, `merge`, `tag`, `remote`, `clone`, `push`, `pull`, `fsck`.
  Default branch `main`; `.gitignore` honored alongside `.agitignore`.
- Content-addressed object model: BLAKE3 ids, zstd-compressed objects
  (`AGO1`), trees (`AGT1`), index (`AGI1`), commits as JSON; every object is
  verified against its id on read.
- `--json` on every command with the `{ok, result|error}` envelope; every
  error carries a stable `code` and a `retryable` flag.
- Commit metadata for agents: `--intent`, `--task`, `--agent`, `--model`,
  `--run-id`, `--meta` (also via `AGIT_AGENT`, `AGIT_MODEL`, `AGIT_RUN_ID`).
- Compare-and-swap ref updates, atomic across several refs; concurrent moves
  fail with a retryable `ref_conflict`.
- Advisory `flock` locking with no stale lock files after a crash; writes go
  through temp + fsync + rename, tunable with `AGIT_FSYNC=refs|all|none`.
- Leases: time-limited, glob-based claims on paths; `commit` refuses to write
  into another holder's exclusive lease.
- Workspaces (many working directories, one object store) and checkpoints
  (disposable snapshots on `refs/checkpoints/<workspace>`).
- Append-only event log `events.jsonl` with sequence numbers and
  `agit events --follow`.
- Submodules via `.agitmodules`.
Added later on 2026-10-15 (still unreleased):

- Reflog for every ref and HEAD move, `agit reflog`, `agit reset
  [--soft|--mixed|--hard]`, `agit gc` with grace period, checkpoint and
  reflog expiry.
- Ed25519 commit signatures (`agit key`, `commit --sign`, `verify`), a
  per-repository key registry and the policy `require_signed`.
- Pack segments (`AGP1` segments with `AGX1` indexes) with publish-on-flush;
  index format `AGI2` with a tree cache; `refs.journal` with group commit.
- Server: scoped tokens bound to agent identities, authorization per
  request, audit log, `/auth/*`; signed webhooks; policies; blame endpoint;
  `commit` events on push; knowledge v2 (BM25 search, staleness and
  validation, drafts from merged pull requests, context packs, linking).
- Local knowledge store in `.agit/knowledge` with offline context and
  `agit know sync`.
- `stash`, `revert`, `cherry-pick`, `blame`, `grep`, `log --graph`, rename
  detection, `push --tags`, `HEAD@{n}` revisions, `agit root`.
- Submodules: `agit submodule add|update|status|remove`, local-path remotes.
- Git bridge: `agit git import` and `agit git export` (round trip verified
  on agit's own history).
- MCP server `agit mcp` with 33 tools; Claude Code hooks and installer in
  `integrations/claude-code`.
- Python SDK (`agit-sdk`) and TypeScript SDK (`@agit/sdk`).
- Web UI: code browsing, commits, pull requests, agents, leases, knowledge,
  settings (policy, webhooks, rules, keys), blame, search, compare,
  notifications.
- Handbook under `docs/handbook` (20 chapters, German overview) published
  as an mdBook site; load test `examples/load.rs`; benchmarks.

### Changed

- Loose objects (`AGO1`) are still read but new objects go into segments.
- Index files are written as `AGI2`; `AGI1` indexes are still loaded.
- `GET /health` is public; every other route needs a token when a root
  token is configured.

- `agit mcp`: a Model Context Protocol server exposing the CLI to agents.

Server (`agit-server`, `crates/agit-server`, `crates/agit-protocol`):

- Repositories over HTTP (`/api/v1`): refs with CAS updates, object upload
  and fetch with want/have negotiation, clone/push/pull, browsing endpoints
  (tree, commits, diff), server-wide leases.
- Event log per repository with polling and a server-sent events stream.
- Pull requests with structured comments, reviews, checks, mergeability
  preview and server-side merge (three-way or fast-forward) with CAS on the
  target branch; `agit pr create|list|view|review|comment|merge|close`.
- Per-repository policies: protected branches, PR-only, minimum approvals,
  required checks, allowed merge/push agents, `require_intent`,
  `require_agent`, `max_files_per_commit`; violations are `403 policy_violation`.
- Webhooks with kind filters, retries and keyed-BLAKE3 signatures
  (`X-Agit-Signature: blake3=<hex>`).
- Knowledge base: notes with typed tags, tag rules, search, and the per-path
  `context` endpoint (notes, recent commits, leases, open PRs);
  `agit know add|list|context`.
- Shared bearer-token authentication (`AGIT_SERVER_TOKEN` / `AGIT_TOKEN`)
  and static hosting of the web UI (`--web-dir`).

Platform:

- Web UI (`web/`): code browsing with syntax highlighting and a file finder,
  commits with agent/intent/task filters, diffs, pull requests with reviews
  and inline comments, live agent activity, a lease board, and the knowledge
  browser; light and dark themes; mock mode (`VITE_AGIT_MOCK=1`).
- Git bridge: `agit git import` (full history, incremental re-import, git
  trailers lifted into `intent`/`task`/`agent`) and `agit git export` via
  `git fast-import`.
- SDKs: `@agit/sdk` (TypeScript, `sdk/typescript`) and `agit-sdk` (Python,
  `sdk/python`) covering the HTTP API and the `--json` CLI.
- Documentation: `docs/API.md` (HTTP contract), `docs/ARCHITECTURE.md`,
  `docs/AGENT-GUIDE.md`, `docs/BENCHMARKS.md`, `docs/ROADMAP.md` and RFCs.

Operations and distribution:

- `Dockerfile` (multi-stage, non-root, `/data` volume, health check) and
  `docker-compose.yml`; images published to `ghcr.io/shadow-kernel/agit`.
- `scripts/install.sh`: checksum-verified installer for macOS and Linux.
- `.github/workflows/release.yml`: tagged releases with binaries for Linux
  (x86_64, aarch64), macOS (x86_64, arm64) and Windows (x86_64), the web UI
  tarball, the container image, and optional npm/PyPI publishing.
- `scripts/release.sh` (version bump + changelog + tag), `homebrew/agit.rb`,
  systemd unit and reverse-proxy examples in `deploy/`,
  `docs/DEPLOYMENT.md` and `docs/SECURITY.md`.

[Unreleased]: https://github.com/shadow-kernel/agit/commits/main
