# agit-server (planned)

Status: 📋 planned (Roadmap Phase 2 and 3). This directory holds only this
README; there is no crate here yet and it is not a workspace member.

`agit-server` will be the hosting server for agit repositories: the component
that turns the local core into a platform.

Planned responsibilities:

- Repositories, organisations, permissions, and short-lived scoped tokens per
  agent.
- Per-repository append-only commit log replicated with Raft; ref
  compare-and-swap decided at the log head.
- Content-addressed blob store with a local disk tier and an S3-compatible
  tier, deduplicated across repositories.
- Horizontal sharding by repository.
- Pull requests as structured objects, structured review comments, merge
  policies (`docs/rfcs/0005-pull-requests-and-reviews.md`).
- Server-side event log with webhooks and SSE/WebSocket streaming.
- Knowledge base indexing and query API (`crates/agit-knowledge`).
- One public API, used by the web UI, the SDKs, and the MCP server alike.

Throughput target: 10k to 20k commits per second per shard for a hot
repository, 100k per second platform-wide. See
`docs/rfcs/0004-throughput.md`. No numbers have been measured yet.

This component is designed to be split into its own repository and linked
back as a submodule (`.agitmodules`).
