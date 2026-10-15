# agit: notes for coding agents

- Rust workspace. `cargo test` runs unit tests in `crates/agit-core/src/*` and end-to-end tests in `crates/agit-core/tests/end_to_end.rs`. Keep both green.
- Crates: `agit-core` (library: object model, store, index, refs, worktree, ops, submodules), `agit-protocol` (API types, object stream, want/have walk, HTTP client, push/fetch/clone sync, submodule commands), `agit-server` (axum server: refs, PRs, policies, leases, webhooks, knowledge, events), `agit-git-bridge` (git import/export via the git CLI), `agit-cli` (thin clap layer incl. `agit mcp`). Business logic belongs in core/protocol/server modules, never in the CLI.
- The HTTP contract is `docs/API.md`; the web UI in `web/` and the client in `agit-protocol` both follow it. Change the spec first, then both sides.
- Server end-to-end test: `cargo test -p agit-server` (starts an in-process server on a random port). `make check` runs fmt, clippy -D warnings and all tests; CI does the same plus web and SDK builds.
- SDKs in `sdk/python` and `sdk/typescript` and the web UI in `web/` must track `docs/API.md`; when adding an endpoint, update the spec, the Rust client, both SDKs and the web client.
- Every CLI command must support `--json` with the `{ok, result|error}` envelope. New error variants need a stable `code()` in `error.rs`.
- On-disk formats (`AGO1` loose objects, `AGP1`/`AGX1` pack segments and indexes, `AGT1` trees, `AGI2` index, commit JSON, `refs.journal`) are documented in `docs/ARCHITECTURE.md`; changing them requires bumping `FORMAT_VERSION` and a note in `docs/rfcs/0001-object-model.md`.
- Performance claims go in `docs/BENCHMARKS.md` only when measured. Everything else is a target.
- Design source of truth: `README.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`, `docs/rfcs/`.
