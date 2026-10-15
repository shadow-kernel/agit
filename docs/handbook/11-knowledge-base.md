# 11. Knowledge base

The knowledge base keeps what a codebase's maintainers know (decisions, gotchas, specs, how-tos) next to the code it describes, in a form that reaches an agent at the moment it is about to change that code. This chapter explains why it is designed this way, how to tag, how context is assembled, how staleness and drafts work, how to write notes that survive, how agents should query, and what the implementation cannot do yet. The data model is summarised in [5. Concepts](05-concepts.md#knowledge-model); the endpoints are in [8](08-http-api.md#knowledge-notes) and [docs/API.md](../API.md); the design target is [RFC 0003](../rfcs/0003-knowledge-base.md).

## Why knowledge lives next to the code

Wikis and chat threads fail agents for a delivery reason, not a content reason: the agent editing `src/auth/session.rs` never sees the wiki page about session rotation. Putting notes in the same system as the code fixes three things at once:

1. **Addressing.** A note is linked to paths (`src/auth/**`), commits and pull requests, so "notes about the file I am editing" is a lookup, not a search.
2. **Validity.** A note records the commit it was written against (`valid_at`). When the covered paths change, the note becomes `stale` and shows up in a list that someone must clear. Knowledge that cannot go stale cannot be trusted.
3. **Provenance.** Every merged pull request leaves a draft note; every note has an author, a confidence and a history of validation. Agents write knowledge as a by-product of working, not as a separate chore.

In 0.1.0 notes are JSON files on the server (`<repo>/knowledge/notes/<id>.json`) plus a local cache and offline store in `.agit/knowledge/`. The RFC's target, immutable note objects on `refs/knowledge/<branch>` that clone, branch and merge with the code, is 📋. The API is designed so that this move will not change how agents use it.

## Tag taxonomy

Tags are `namespace:value`. Keep the set small and predictable; agents should be able to guess a tag without looking it up.

| Namespace | Use for | Examples | Guidance |
| --- | --- | --- | --- |
| `area:` | module or domain | `area:auth`, `area:billing`, `area:core`, `area:infra` | one per top-level module; assign through tag rules so nobody has to remember |
| `kind:` | what kind of knowledge | `kind:decision`, `kind:gotcha`, `kind:spec`, `kind:howto`, `kind:fact`, `kind:rule`, `kind:change` | exactly one per note; `kind:change` is reserved for drafts generated from merges |
| `lang:` | language or runtime | `lang:rust`, `lang:typescript`, `lang:sql` | from rules (`**/*.rs → lang:rust`) |
| `service:` | deployable unit, when it differs from the area | `service:api`, `service:worker` | optional; useful in monorepos |
| anything else | your own axes | `team:payments`, `sev:high`, `ticket:SEC-12` | allowed and indexed; document them in a `kind:rule` note |

The server validates tag length and characters; it currently also accepts a bare value without a namespace, which the documentation says it should reject. Do not rely on that: a tag without a namespace cannot be targeted by rules and will not match `area:`/`kind:` filters.

## Tag rules

Rules map path patterns to tags and are the glue between paths and notes. `GET/PUT /knowledge/rules` (admin), also editable in the web UI under Settings, Knowledge rules:

```json
[{"pattern":"src/**","tags":["area:core","lang:rust"]},{"pattern":"vendor/**","tags":["area:vendor"]}]
```

Two questions they answer:

- *Which tags describe this path?* The union over matching rules. The context call returns it as `suggested_tags`, and the Claude Code pre-edit hook injects it as "tags for notes about this path".
- *Which tags should a draft note carry?* The merge drafter adds the rules' tags for every changed path to `kind:change`.

Recommended starting set for a Rust service repository:

```json
[
  {"pattern":"**/*.rs","tags":["lang:rust"]},
  {"pattern":"src/auth/**","tags":["area:auth","service:api"]},
  {"pattern":"src/billing/**","tags":["area:billing","service:api"]},
  {"pattern":"migrations/**","tags":["area:db","kind:rule"]},
  {"pattern":"deploy/**","tags":["area:infra"]}
]
```

Keep rules coarse (directories, not files) and revisit them when the tree is reorganised; stale rules produce stale suggestions, which produce badly tagged notes.

## How context is assembled

`GET /context?path=…` (CLI `agit know context`, MCP `agit_context`, SDK `context`/`context_pack`) is one call on purpose: it is what an agent should read before changing a path, and it has to be cheap enough to call every time. The server:

1. finds notes whose `paths` cover the requested path (a note on `src/**` covers `src/config.rs`; a note on `src/config.rs` covers only that file), excluding drafts, newest update first, then **sorts fresh notes before stale ones** and cuts at `limit` (default 10);
2. computes `suggested_tags` from the rules;
3. walks the default branch history for commits touching the path (`recent_commits`, up to `limit`);
4. lists `leases` whose patterns overlap the path (in either direction);
5. lists open pull requests whose changes touch the path (`open_pulls`);
6. derives `agents_active`: lease holders plus the agents (or authors) who committed to the path in the last 24 hours;
7. with `include=files`, appends the file itself and up to five siblings, most recently touched first, trimmed last-first to keep the whole response within roughly `budget * 4` characters (`budget` defaults to 6000 tokens; `truncated: true` marks a cut file).

Note bodies are never part of a context pack, only 200-character excerpts, so the pack stays small; fetch the full note when an excerpt is relevant. In the walkthrough, three notes, two commits and the active agents for `src/config.rs` fit in under 3 KB.

```
$ agit know context src/config.rs
context for src/config.rs
suggested tags: area:core, lang:rust

notes (4):
  n_15761d1fc397 Config is read once at startup [kind:fact, area:core]
  n_5a5d76aedccb Greeting text lives in Config, not main.rs [kind:decision, area:core]
  n_599ca51c9760 PR #1: greeting moved into Config [kind:change, area:core, lang:rust]
  n_1f5d3dce7c3b Config::load() reads GREETING from the environment [kind:gotcha, area:core, lang:rust]

recent commits (4): …
leases (0):
open pull requests (0):
agents active here: coder-1, reviewer-1
```

Commit detail (`GET /commits/{rev}`) links back: up to 10 notes that list the commit (drafts included) or whose paths cover a changed path.

## Staleness and the validation workflow

`valid_at` is the commit a note was written or last confirmed against (default: HEAD of the default branch at creation; `valid_at` can be given explicitly and must exist on the server). `stale` is computed on every read: `true` when any path covered by `paths` differs between `valid_at` and HEAD. The per-commit-pair diff is cached in memory. Notes without paths or without `valid_at` are never stale.

The workflow, as run in [chapter 4](04-quickstart-agents.md#12-validate-stale-notes):

1. A merge changes `src/config.rs`. Every note covering it is now stale; `GET /knowledge/stale` lists them with `changed_paths`, the context call sorts them after fresh notes and marks `stale: true`, the web UI shows a badge.
2. The agent (ideally the one who made the change, as part of finishing the task) reads each stale note and decides:
   - still true: `POST /notes/{id}/validate` (moves `valid_at` to HEAD, bumps `updated_ms`);
   - wrong: `PATCH` the title/body first, then `validate` (editing alone does not clear staleness);
   - obsolete: `DELETE`, or create a replacement and set `superseded_by` on the old one (superseded notes are hidden unless `include_superseded=1`).
3. The event stream carries `note.validate`, `note.update` and `note.delete`, so a dashboard or a nightly "knowledge gardener" agent can watch the backlog.

A policy of "no stale notes on the paths you touched when you open the PR" is a good team rule; the server does not enforce it yet.

## Drafts from merges

Every successful `POST /pulls/{n}/merge` creates a draft note:

| Field | Value |
| --- | --- |
| `title` | the PR title |
| `body` | `Intent:` and `Task:` lines, the PR body, a `Changed files:` list (`- path (A|M|D|T)`), a `Reviews:` list with each reviewer's latest decision |
| `tags` | `kind:change` plus the rules' tags for the changed paths |
| `paths` | the changed files' directories as `dir/**` (root files by name), deduplicated |
| `pulls`, `commits` | `[n]`, `[merged_commit]` |
| `author`, `confidence`, `valid_at` | the merging agent, `0.4`, the merged commit |

Drafts are hidden from listings, context and tag counts until accepted. Find them with `GET /knowledge/notes?drafts=1&pull=<n>` (or `drafts=1` alone), then `POST /notes/{id}/accept` with an optional patch (retitle, raise confidence, trim the body to the durable part) or `DELETE`. There is no switch to turn drafting off; an unreviewed draft costs nothing because nobody sees it.

```json
{"id":"n_599ca51c9760","title":"Load greeting from Config","draft":true,"tags":["kind:change","area:core","lang:rust"],"paths":["src/**"],"confidence":0.4}
```

Accept drafts that record a decision or a non-obvious consequence; delete drafts of routine changes. A draft that is accepted unchanged is rarely useful; a one-line edit that states *why* usually is.

## Writing good notes

A note should be true, specific, and actionable for the next agent. Templates that work:

**Gotcha** (`kind:gotcha`): one surprising fact and what to do about it.

```
title: Session store is eventually consistent
tags:  kind:gotcha area:auth service:api
paths: src/auth/session.rs src/auth/store/**
body:  Reads after a token rotation can lag by up to 200 ms on the replica.
       Retry once with a 250 ms backoff before treating a missing token as revoked.
       Seen in SEC-12; test in tests/auth/rotation.rs.
```

**Decision** (`kind:decision`): what was decided, the alternatives, the consequence; link the PR.

```
title: Greeting text lives in Config, not main.rs
tags:  kind:decision area:core
paths: src/**
pulls: 1
body:  Decided in PR #1: main.rs only prints; the text comes from Config so that
       DEMO-8 can source it from the environment. Alternative (format string in main)
       rejected because localisation will need a locale-aware source.
```

**Spec** (`kind:spec`): the contract a module must keep, phrased as checkable statements.

```
title: Webhook signature contract
tags:  kind:spec area:webhooks
paths: crates/agit-server/src/webhooks.rs
body:  X-Agit-Signature is blake3=<hex(keyed_hash(derive_key("agit webhook v1", secret), body))>.
       Receivers must compare in constant time and dedupe on X-Agit-Delivery.
       Retries: 3, backoff 200 ms x4.
```

**How-to** (`kind:howto`): numbered steps with the exact commands.

```
title: Rotate the server token
tags:  kind:howto area:infra
paths: deploy/**
body:  1. openssl rand -hex 32 > new token  2. update deploy/agit-server.env
       3. systemctl restart agit-server  4. update AGIT_TOKEN in every agent env
       There is no overlap window; expect 403s for a few seconds.
```

Rules of thumb:

- Title is a claim, not a topic: "Config::load() reads GREETING from the environment", not "Config".
- Scope `paths` as narrowly as the claim: a note about one function should name the file, a note about a module convention should use `dir/**`. Over-broad paths make notes stale on every commit and bury them in every context pack.
- Link `--commit HEAD` (or the PR) so readers can see the code the note was written against.
- Set `confidence` honestly (`0.9` for verified behaviour, `0.5` for a hypothesis); the context call does not rank by it yet, but readers see it.
- One `kind:`, one or two `area:`, plus the suggested tags. Do not invent namespaces per note.
- Prefer updating and validating an existing note over writing a near-duplicate; search first (`agit know search -q "..." --path <path>`).
- Keep bodies under a few hundred words; put long material in the repository and link the path.

## Querying strategies for agents

| Question | Call |
| --- | --- |
| What should I read before touching `path`? | `agit know context <path>` (always; it is cheap) |
| Is there anything known about this symptom? | `agit know search -q "<symptom words>" --path <path>` (BM25, best first; prefix matching helps with identifiers) |
| All gotchas in my area | `agit know search --tag kind:gotcha --tag area:auth` (all tags must match) |
| What changed knowledge-wise since my last run? | `GET /events?since=<seq>` filtered on `note.*`, or `agit events --kind note.` on the server log |
| Which notes did this commit or PR produce? | `GET /knowledge/notes?commit=<rev>` / `?pull=<n>`; `GET /commits/{rev}` returns `notes` |
| What do I owe the knowledge base after merging? | `GET /knowledge/stale`, `GET /knowledge/notes?drafts=1&pull=<n>` |
| What tags does this repository use? | `agit know tags` |

Search tokenisation: lowercase alphanumeric runs; camelCase and snake_case identifiers are indexed whole and split (`SessionStore` matches `session`, `store` and `sessionstore`); `namespace:value` tags are indexed whole and split; terms of three or more characters also match as prefixes at half weight. Field weights: title 3, tags 2, body and paths 1. `tags`, `path`, `commit` and `pull` are post-filters applied in rank order. Without `q`, results are ordered by `updated_ms` descending.

Budget tips: ask for `limit=5` when you only need the top notes; use `include=files&budget=<n>` to get file texts in the same round trip instead of separate reads; never fetch full bodies in a loop, fetch the one or two whose excerpt matters.

## Offline: local store and sync

`agit know` and the MCP knowledge tools are server-first with a local fallback:

- With a reachable remote, every call goes to the server and the result carries `source: "server"`.
- On a transport error (server down, wrong port, no network) or without a remote, the call is answered from `.agit/knowledge/` and carries `source: "local"`. Notes created locally get ids `n_local_<hex>`.
- `--local` forces the local store.
- `agit know sync` uploads local-only notes to the server (they receive normal `n_` ids) and refreshes the local cache of server notes and tag rules; run it when connectivity returns.

```
$ agit know --remote broken search --tag kind:gotcha --json | jq -c '{source: .result.source, n: (.result.notes|length)}'
{"source":"local","n":1}
$ agit know --local add -t "Offline thought" --tag kind:note --path src/main.rs -b "written while offline" --json | jq -c '.result | {id: .note.id, source}'
{"id":"n_local_223c4ac7cc","source":"local"}
$ agit know sync --json
{"ok":true,"result":{"pushed":1,"pulled":9,"rules_pulled":2}}
```

The local store computes `suggested_tags` from the cached rules and `recent_commits` from the local history, so a context pack offline is a reasonable approximation of the server's.

## Limits today

- **No stemming, no synonyms, no embeddings.** "rotate" does not match "rotation" unless by prefix. Semantic search is 📋.
- **No ranking by confidence or recency** in the context pack beyond "fresh before stale, newest first".
- **Notes are mutable JSON**, not versioned objects; there is no history of a note's edits beyond `updated_ms` and the event log. `refs/knowledge/*`, per-branch knowledge and merging notes with code are 📋 (RFC 0003).
- **Links are paths, commits and pulls**; `symbol`, `note` and `url` links from the RFC do not exist. Supersession is the mutable `superseded_by` field.
- **Staleness is per path, not per claim**: any change to a covered file marks the note stale, including a comment fix. Narrow paths keep the noise down.
- **One knowledge base per repository**; nothing is shared across repositories.
- **Tag namespaces are a convention**, not enforced server-side.
- **Index in memory**: the BM25 index is built per repository at the first `q=` query after start and kept current on writes; the first search after a restart pays for the build.
