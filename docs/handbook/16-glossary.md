# 16. Glossary

Terms as agit uses them, with the chapter that explains each.

| Term | Meaning | See |
| --- | --- | --- |
| **agent** | A program (usually model-driven) acting in a repository, identified by a stable id (`AGIT_AGENT`, `X-Agit-Agent`) and optionally `model`, `run_id`, `on_behalf_of`. Recorded on commits, leases, PRs, reviews and notes. Self-asserted unless a bound token is used. | [4](04-quickstart-agents.md), [13](13-server-administration.md#scoped-tokens) |
| **AGO1** | Magic of a loose object file: `AGO1 | kind | raw_len | zstd`. Legacy format, still read. | [14](14-architecture-internals.md#formats) |
| **AGP / segment** | Append-only pack segment file (`objects/pack/seg-<pid>-<rand>.agp`) holding object records; published through an `AGX1` index. | [14](14-architecture-internals.md#pack-segments-agp1-files-with-agx1-indexes) |
| **AGT1** | Binary tree format with mode, id, size and name per entry. | [14](14-architecture-internals.md#trees-agt1) |
| **AGI2** | Index (staging area) format with stat cache and cached tree ids; `AGI1` is read. | [14](14-architecture-internals.md#index-agi2) |
| **AGX1** | Pack segment index: sorted `(id, offset, comp_len, raw_len, kind)` entries plus checksum. | [14](14-architecture-internals.md) |
| **audit log** | `<data-dir>/audit.jsonl`: every mutation and every denied request with principal, agent, method, path, status. | [13](13-server-administration.md#audit-log) |
| **blob, tree, commit** | The three object kinds; ids are BLAKE3 over a kind tag plus payload. | [5](05-concepts.md#objects-and-ids) |
| **branch** | A ref under `refs/heads/`. Workspace branches are `ws/<name>`. | [5](05-concepts.md#refs-and-compare-and-swap) |
| **CAS, compare-and-swap** | Ref update that states the expected current value (`any`, `absent`, `is`); failure is `ref_conflict`. | [5](05-concepts.md#refs-and-compare-and-swap) |
| **check** | A status reported on a PR (`pending`, `success`, `failure`, with name and URL), typically by a CI agent; `require_checks` makes them mandatory. | [8](08-http-api.md#pull-request-lifecycle) |
| **checkpoint** | Snapshot of the whole working tree (untracked included) as a commit with `checkpoint: true` on `refs/checkpoints/<workspace>`; outside branch history. | [5](05-concepts.md#checkpoints) |
| **common directory** | `.agit/` of the primary workspace, shared by all workspaces of a repository. | [5](05-concepts.md#workspaces) |
| **confidence** | A note's self-assessed certainty, 0 to 1 (default 1; drafts 0.4). | [11](11-knowledge-base.md) |
| **context pack** | The result of `GET /context?path=` / `agit know context`: notes, suggested tags, recent commits, leases, open PRs, active agents and optionally budgeted file texts. | [11](11-knowledge-base.md#how-context-is-assembled) |
| **draft note** | A note generated from a merged PR, hidden until accepted or deleted. | [11](11-knowledge-base.md#drafts-from-merges) |
| **durability level** | `AGIT_FSYNC=refs|full|all|none`. | [5](05-concepts.md#pack-segments-and-durability) |
| **envelope** | `{"ok":true,"result":…}` / `{"ok":false,"error":{code,message,retryable,details}}`, printed by every command with `--json`. | [6](06-cli-reference.md#the-json-contract) |
| **event** | One JSON line in `events.jsonl` with `seq`, `time_ms`, `kind`, `workspace?`, `actor?`, `data`. | [5](05-concepts.md#events) |
| **exit codes** | 0 ok, 1 error, 3 conflict, 75 retryable. | [6](06-cli-reference.md#exit-codes-and-error-codes) |
| **flock** | Advisory kernel file lock used instead of `.lock` sentinel files. | [14](14-architecture-internals.md#locking-and-concurrency) |
| **git bridge** | `agit git import` / `agit git export`, conversion through the `git` CLI with a SHA mapping table. | [14](14-architecture-internals.md#git-bridge-mapping) |
| **intent** | Structured commit field answering "why"; `--intent`. | [5](05-concepts.md#commits-and-metadata) |
| **key registry** | Per-repository list of trusted Ed25519 public keys per agent (`/keys`, `keys.json`). | [13](13-server-administration.md#signing-keys) |
| **lease** | Advisory, time-limited claim by a holder on path patterns, `exclusive` or `shared`; enforced at commit time. | [5](05-concepts.md#leases) |
| **local knowledge store** | `.agit/knowledge/`: offline notes (`n_local_*`) and a cache of server notes and rules; `agit know sync`. | [11](11-knowledge-base.md#offline-local-store-and-sync) |
| **MCP** | Model Context Protocol; `agit mcp` exposes 33 tools over stdio. | [7](07-mcp-reference.md) |
| **merge base** | The nearest common ancestor chosen by a bounded breadth-first walk. | [14](14-architecture-internals.md#merge-algorithm) |
| **meta** | Namespaced JSON map on a commit (`ci.tests`, `git.sha`, `agit.previous_checkpoint`); `--meta k=v`. | [5](05-concepts.md#commits-and-metadata) |
| **note** | A knowledge entry: title, markdown body, typed tags, paths, commits, pulls, author, confidence, `valid_at`, `stale`, `draft`. | [11](11-knowledge-base.md) |
| **object stream** | Wire framing `kind:u8 | len:u32be | payload` used by push and fetch. | [8](08-http-api.md#objects-and-object-streams) |
| **open mode** | A server without `AGIT_SERVER_TOKEN`; every request acts as an anonymous root. | [13](13-server-administration.md#authentication) |
| **policy** | Per-repository rules for protected branches, enforced on pushes and PR merges. | [5](05-concepts.md#policies) |
| **primary workspace** | The workspace named `main`, whose working directory contains `.agit/` as a directory. | [5](05-concepts.md#workspaces) |
| **publish-on-flush** | Objects become visible to other processes only when the segment index is published, which happens before every ref update and index save. | [14](14-architecture-internals.md#the-publish-on-flush-rule) |
| **pull request (PR)** | Server-side object with source/target refs, intent, task, author, typed comments, reviews, checks and state `open|merged|closed`. | [8](08-http-api.md#pull-request-lifecycle) |
| **reflog** | Append-only log of where a ref or HEAD pointed; basis for `agit reflog` and undoing `reset`. | [5](05-concepts.md#reflog) |
| **remote** | Named server URL of the form `…/api/v1/repos/<name>`. | [5](05-concepts.md#remotes) |
| **retryable** | Error flag meaning the identical call may succeed if repeated (`ref_conflict`, `lock_timeout`, transport errors). | [6](06-cli-reference.md#exit-codes-and-error-codes) |
| **review** | A verdict on a PR by a reviewer: `approve`, `request_changes`, `comment`; the latest per reviewer counts. | [12](12-multi-agent-workflows.md#reviewer-agents) |
| **root token** | `AGIT_SERVER_TOKEN`; full access, mints scoped tokens. | [13](13-server-administration.md#the-root-token) |
| **scoped token** | Token from `POST /auth/tokens` with `{repo, access}` scopes, optional agent binding and expiry. | [13](13-server-administration.md#scoped-tokens) |
| **SSE** | Server-Sent Events stream of a repository's events, `GET /events/stream?since=`. | [8](08-http-api.md#events-and-sse) |
| **stale** | A note whose covered paths changed since `valid_at`; computed on read. | [11](11-knowledge-base.md#staleness-and-the-validation-workflow) |
| **stash** | Shelved working tree changes (tracked and untracked) applied back as a three-way merge. | [3](03-quickstart-developers.md#stash) |
| **submodule** | Tree entry of mode `submodule` pinning a commit of a linked repository described in `.agitmodules`. | [5](05-concepts.md#submodules) |
| **suggested tags** | Tags the rules assign to a path; returned by the context call. | [11](11-knowledge-base.md#tag-rules) |
| **tag (ref)** | A ref under `refs/tags/`; local only in 0.1.0. | [6](06-cli-reference.md) |
| **tag (knowledge)** | `namespace:value` label on a note (`area:`, `kind:`, `lang:`, `service:`). | [11](11-knowledge-base.md#tag-taxonomy) |
| **tag rule** | `{pattern, tags}` mapping a path glob to tags. | [11](11-knowledge-base.md#tag-rules) |
| **task** | Structured commit and PR field linking a ticket; `--task`. | [5](05-concepts.md#commits-and-metadata) |
| **valid_at** | The commit a note was written or last validated against. | [11](11-knowledge-base.md) |
| **webhook** | Subscription delivering events to an HTTP endpoint with retries and a keyed-BLAKE3 signature. | [8](08-http-api.md#webhooks) |
| **workspace** | A working directory with its own HEAD and index sharing the common directory; one per agent; branch `ws/<name>`. | [5](05-concepts.md#workspaces) |
