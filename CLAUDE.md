# agit: notes for coding agents

- Rust workspace. `cargo test` runs unit tests in `crates/agit-core/src/*` and end-to-end tests in `crates/agit-core/tests/end_to_end.rs`. Keep both green.
- `crates/agit-core` is the library (object model, store, index, refs, worktree, ops). `crates/agit-cli` is a thin clap layer; business logic belongs in `ops.rs` or the module it concerns, never in the CLI.
- Every CLI command must support `--json` with the `{ok, result|error}` envelope. New error variants need a stable `code()` in `error.rs`.
- On-disk formats (`AGO1`, `AGT1`, `AGI1`, commit JSON) are documented in `docs/ARCHITECTURE.md`; changing them requires bumping `FORMAT_VERSION` and a note in `docs/rfcs/0001-object-model.md`.
- Performance claims go in `docs/BENCHMARKS.md` only when measured. Everything else is a target.
- Design source of truth: `README.md`, `docs/ARCHITECTURE.md`, `docs/ROADMAP.md`, `docs/rfcs/`.
