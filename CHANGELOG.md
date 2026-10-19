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

## [0.1.1] - 2026-10-19

### Fixed

- Repository discovery no longer mistakes the per-user `~/.agit` directory
  (signing keys, cached sessions) for a repository. Commands run in a
  directory below the home directory that is not inside a repository failed
  with `io error at ~/.agit/HEAD`; they now report `not_a_repository`. A
  `.agit` directory counts as a repository only when it holds `HEAD` or
  `config.json`. `agit init` in the home directory explains the clash instead
  of answering `already_exists`, `agit git import --into <dir>` opens exactly
  that directory, and the Claude Code hooks apply the same rule.

### Changed

- The handbook is no longer published as a separate site; the website is
  https://shadow-kernel.github.io/agit-site/ and the Docs workflow only checks
  that the handbook builds. New README with the website, badges and a short
  tour.

## [0.1.0] - 2026-10-19

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

Added on 2026-10-19:

- SSH-key authentication: the server keeps authorized Ed25519 public keys
  with per-repository scopes (`/auth/keys`), clients sign a single-use
  challenge (`/auth/challenge`, `/auth/session`) and receive a session
  token; `--admin-key` / `AGIT_ADMIN_KEYS` make a server closed with your
  own `~/.ssh/id_ed25519.pub` as the admin. `agit auth login|logout|whoami|
  show-token|key|token`; remote commands sign in on their own after a
  `401`, with passphrase-protected keys through ssh-agent; sessions cached
  in `~/.agit/sessions.json`. `POST /auth/logout`. Web UI admin page for
  keys; SDK methods (`login_with_key`, `loginWithKey`, tokens, keys).
- Durability level `AGIT_FSYNC=full` (full flush of the ref journal, power-
  loss safe on macOS); every fsync goes through `durability::sync_file`.

### Changed

- Loose objects (`AGO1`) are still read but new objects go into segments.
- Index files are written as `AGI2`; `AGI1` indexes are still loaded.
- `GET /health` is public; every other route needs a token when a root
  token is configured.
- Missing, invalid, expired or revoked credentials are `401 unauthorized`
  (new error code; `403 forbidden` now only means "valid credentials, wrong
  scope"). `agit-server` refuses to start without any authentication on a
  non-loopback address unless `--allow-open` is given. Server-wide (`*`)
  scopes can be granted by any `*:admin` principal, not only root.
- `AGIT_FSYNC=refs` (the default) now syncs with `fsync(2)` exactly like
  git. On Linux nothing changes; on macOS the standard library's
  `F_FULLFSYNC` is no longer issued per commit, which makes a commit at
  default durability several times faster (numbers in BENCHMARKS.md) at the
  cost of power-loss safety, which `AGIT_FSYNC=full` restores.

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

[Unreleased]: https://github.com/shadow-kernel/agit/compare/v0.1.1...HEAD
[0.1.1]: https://github.com/shadow-kernel/agit/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/shadow-kernel/agit/releases/tag/v0.1.0
