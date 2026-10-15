# Contributing to agit

Thanks for your interest. agit is early (Phase 0, see `docs/ROADMAP.md`), so
the most valuable contributions right now are bug reports against the local
core, format and API review of the RFCs, and tests.

## Ground rules

- Read `docs/ARCHITECTURE.md` before touching `crates/agit-core`. The on-disk
  formats (`AGO1`, `AGT1`, `AGI1`, commit JSON) are not frozen yet, but any
  change to them needs an RFC update in `docs/rfcs/` in the same pull request.
- Every CLI command must support `--json`, and every error must map to a stable
  `code` and a `retryable` flag. New error codes are documented in
  `README.md` under "For AI agents".
- No silent data loss. Writes go through temp + fsync + rename, refs through
  compare-and-swap. If you need to bypass that, explain why in the PR.
- No `.lock` sentinel files. Use `agit_core::lock::Lock` (advisory `flock`).
- Prefer deterministic output: sorted keys, sorted paths, stable ordering.

## Workflow

```sh
cargo build
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

Until agit is self-hosting, this repository is managed with git. Use small,
focused commits with a message that states the intent of the change, the same
thing `agit commit --intent` will eventually record for you.

Open a pull request against `main`. Describe what changed, why, and how you
tested it. Link the RFC or issue when one exists.

## Design changes

Anything that changes a format, a wire protocol, a CLI contract, or a core
semantic (leases, CAS, events) goes through an RFC:

1. Copy the structure of an existing file in `docs/rfcs/`.
2. Use the next free number. Status starts at `Draft`.
3. Open a PR; discussion happens on the PR. When merged with
   `Status: Accepted`, the design is binding until superseded.

## Commits and provenance

If you are an AI agent contributing to agit, say so. Put the agent id and
model in the commit trailer until agit's own `--agent`/`--model` metadata can
be used here:

```
Agent: coder-7
Model: <model name>
```

## License

By contributing you agree that your contributions are licensed under both the
MIT license and the Apache License 2.0, at the user's option, like the rest of
the project (`LICENSE-MIT`, `LICENSE-APACHE`).
