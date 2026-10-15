# agit-protocol

Shared wire types and client logic for the agit server API ([docs/API.md](../../docs/API.md)).

- `types.rs` JSON request/response types (refs, objects, pull requests, knowledge, context, agents)
- `stream.rs` object stream format `kind:u8 | len:u32be | payload`, import/export against an `ObjectStore`
- `walk.rs` reachability: which objects must travel for a push or fetch (want/have)
- `client.rs` blocking HTTP client (`ureq`) with the same error codes as the CLI
- `sync.rs` push (compare-and-swap on the remote ref, non-fast-forward protection), fetch into `refs/remotes/<remote>/`, pull (fetch + merge), clone

Used by `agit-cli` for `remote`, `push`, `fetch`, `pull`, `clone`, `pr` and `know`, and by the server tests.
