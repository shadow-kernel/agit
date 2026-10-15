# agit-server

Single-node hosting server for agit repositories. Implements [docs/API.md](../../docs/API.md).

## Run

```sh
cargo run --release -p agit-server -- --data-dir ./agit-data --listen 127.0.0.1:7878
# with auth and the web UI:
AGIT_SERVER_TOKEN=secret agit-server --data-dir ./agit-data --web-dir web/dist
```

Flags and environment: `--data-dir` / `AGIT_DATA_DIR`, `--listen` / `AGIT_LISTEN`, `--token` / `AGIT_SERVER_TOKEN`, `--web-dir` / `AGIT_WEB_DIR`, `RUST_LOG` for tracing.

## What it stores

`<data-dir>/repos/<name>/` is a bare agit repository plus platform data:

```
meta.json           name, default branch, created
objects/            content-addressed store (shared code with agit-core)
refs/               heads, tags; every update is compare-and-swap
events.jsonl        append-only event log (also fanned out over SSE)
leases/             server-wide path leases
pulls/<n>.json      pull requests with reviews, comments, checks
knowledge/notes/    knowledge notes; knowledge/rules.json tag rules
```

## Layout

- `src/api.rs` routes and handlers
- `src/state.rs` repository registry and per-repo handles
- `src/pulls.rs` pull request store and server-side merge
- `src/knowledge.rs` notes, tags, rules
- `src/error.rs` error to HTTP mapping (same codes as the CLI)
- `tests/server_e2e.rs` end-to-end test: push, clone, PR merge, pull, knowledge, leases, events

## Planned

Raft-replicated commit log per repository, blob tiers, sharding, organisations and scoped tokens, policies, webhooks. See [RFC 0004](../../docs/rfcs/0004-throughput.md) and [RFC 0005](../../docs/rfcs/0005-pull-requests-and-reviews.md).
