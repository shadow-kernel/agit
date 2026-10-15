# 8. HTTP API: a guided tour

[docs/API.md](../API.md) is the normative contract for `agit-server`; the Rust client, both SDKs and the web UI follow it. This chapter does not repeat every endpoint. It explains the conventions, walks through each area with `curl` calls that were run against a local server, and points at the exact section of API.md for the details.

Base path: `/api/v1`. All ids are 64-hex BLAKE3. Success bodies are plain JSON (no envelope); error bodies share the CLI envelope. Examples use `U=http://127.0.0.1:17979/api/v1` and `H='Authorization: Bearer handbook-demo-token'`.

## Authentication and identity

Two headers matter:

- `Authorization: Bearer <token>`: the root token (`AGIT_SERVER_TOKEN`) or a scoped token from `POST /auth/tokens`. Required on every request under `/api/v1` when a root token is configured. Without a root token the server runs in *open mode* (every request is an anonymous root) for local development.
- `X-Agit-Agent: <id>`: the acting agent, recorded in events, leases, PRs, reviews and notes. A token bound to an agent overrides this header. (The CLI sends `AGIT_AGENT`; `X-Agit-Model` is also read into PR authorship.)

```sh
$ curl -s -i $U/repos | head -1
HTTP/1.1 403 Forbidden
{"ok":false,"error":{"code":"forbidden","message":"missing or invalid bearer token","retryable":false}}
$ curl -s -H "$H" $U/auth/whoami
{"principal":"root","root":true,"scopes":[]}
```

`GET /health` is served **without** a token (`{"status":"ok","version":"0.1.0"}`), which is convenient for load balancers; DEPLOYMENT.md still describes it as protected.

Scoped tokens (admin only to create; the plaintext is returned once):

```sh
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/auth/tokens \
    -d '{"name":"coder-1 laptop","agent":"coder-1","scopes":[{"repo":"demo","access":"write"}],"ttl_secs":3600}'
{"token":"agit_da83a014…","id":"tok_803401c4e6","name":"coder-1 laptop","agent":"coder-1","scopes":[{"repo":"demo","access":"write"}],"created_ms":…,"expires_ms":…,"created_by":"root"}
$ curl -s -H "Authorization: Bearer agit_da83a014…" $U/auth/whoami
{"principal":"tok_803401c4e6","agent":"coder-1","root":false,"scopes":[{"repo":"demo","access":"write"}]}
$ curl -s -X PUT -H "Authorization: Bearer agit_da83a014…" -H 'content-type: application/json' $U/repos/demo/policy -d '{}'
{"ok":false,"error":{"code":"forbidden","message":"token lacks Admin access to demo","retryable":false}}
```

Access needed per request: `GET` is read, other methods are write, and repository create/delete, `policy`, `keys`, `webhooks`, `knowledge/rules` and `/auth/*` are admin. Every mutation and every denied request is appended to `<data-dir>/audit.jsonl`:

```json
{"time_ms":1792093214018,"principal":"tok_803401c4e6","agent":"coder-1","method":"PUT","path":"/repos/demo/policy","status":403}
```

`GET /auth/tokens` lists tokens (hashes only), `DELETE /auth/tokens/{id}` revokes; a revoked token gets `403` immediately. One caveat observed: the bound agent replaces the `X-Agit-Agent` header, but a `holder` field inside a request body (`POST /leases`) is taken as given, so bound identity is not yet enforced on lease holders.

## The error envelope

```json
{"ok":false,"error":{"code":"ref_conflict","message":"…","retryable":true,"details":{"name":"refs/heads/scratch","expected":null,"actual":"3bf5d89c…"}}}
```

HTTP status follows the code: 404 `not_found`/`unknown_revision`/`ref_not_found`/`not_a_repository`, 409 `ref_conflict`/`lease_conflict`/`merge_conflicts`/`already_exists`, 412 `object_not_found`, 400 `invalid`/`invalid_ref_name`/`ambiguous_revision`, 403 `forbidden`/`policy_violation`, 503 `lock_timeout`, 500 otherwise. Codes and `retryable` are identical to the CLI, so one error handler serves both.

```sh
$ curl -s -H "$H" $U/repos/nope
{"ok":false,"error":{"code":"not_a_repository","message":"not an agit repository (or any parent): nope","retryable":false}}
$ curl -s -H "$H" $U/repos/demo/commits/doesnotexist
{"ok":false,"error":{"code":"unknown_revision","message":"unknown revision: doesnotexist","retryable":false}}
```

Malformed JSON bodies are rejected by the framework with `422` and a plain-text message (not the envelope), for example `updates[0].new: invalid object id`.

## Repositories and refs

```sh
$ curl -s -H "$H" $U/repos/demo
{"name":"demo","default_branch":"main","head":"44b9a0f2…","created_ms":1792092720604,"branches":1,"tags":0,"objects":33}
$ curl -s -H "$H" "$U/repos/demo/refs?prefix=refs/heads/"
[{"name":"refs/heads/main","id":"44b9a0f2…"}]
$ curl -s -H "$H" $U/repos/demo/branches
[{"name":"feature/config","id":"edf2c5ac…","ahead":0,"behind":1},{"name":"main","id":"3bf5d89c…","ahead":0,"behind":0}]
```

`POST /repos {name, default_branch?}` creates (409 `already_exists`), `DELETE /repos/{repo}` removes. Use a directory prefix for `refs?prefix=`; a prefix that names a leaf (`refs/heads/main`) currently returns an `io` error.

### CAS semantics

`POST /repos/{repo}/refs/update` takes a list of updates, each with `expect` (`{"any":true}`, `{"absent":true}` or `{"is":"<id>"}`) and `new` (an id, or `null` to delete), plus an optional `message`. All updates succeed or none does; every object referenced by a `new` value must already exist (412 `object_not_found`); protected branches are checked against the policy.

```sh
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/refs/update \
    -d '{"updates":[{"name":"refs/heads/scratch","expect":{"absent":true},"new":"3bf5d89c…"}],"message":"create scratch"}'
{"updated":[{"name":"refs/heads/scratch","id":"3bf5d89c…"}],"deleted":[]}
$ curl -s -i -X POST … -d '{"updates":[{"name":"refs/heads/scratch","expect":{"absent":true},"new":"3bf5d89c…"}]}' | sed -n '1p;$p'
HTTP/1.1 409 Conflict
{"ok":false,"error":{"code":"ref_conflict","message":"reference refs/heads/scratch changed concurrently (expected None, found Some(ObjectId(3bf5d8…)))","retryable":true,"details":{"name":"refs/heads/scratch","expected":null,"actual":"3bf5d89c…"}}}
$ curl -s -X POST … -d '{"updates":[{"name":"refs/heads/scratch","expect":{"is":"3bf5d89c…"},"new":null}]}'
{"updated":[],"deleted":["refs/heads/scratch"]}
```

On a conflict, `details.actual` tells you the current value; re-read, reconcile, retry. A direct update of a protected branch under `require_pull_request` is `403 policy_violation` regardless of `expect`.

## Objects and object streams

Objects travel in a simple framed stream, content type `application/x-agit-objects`: repeated `kind:u8 | len:u32 big-endian | payload`, kinds 1 blob, 2 tree, 3 commit. The server re-hashes every object it receives.

```sh
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/objects/missing \
    -d '{"ids":["3bf5d89c…","0000…0002"]}'
{"missing":["0000000000000000000000000000000000000000000000000000000000000002"]}
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/fetch \
    -d '{"wants":["3bf5d89c…"],"haves":[]}' -o fetch.bin && ls -l fetch.bin | awk '{print $5" bytes"}'
7129 bytes
$ python3 -c "d=open('fetch.bin','rb').read(); print(d[0], int.from_bytes(d[1:5],'big'))"
2 112                        # first frame: a tree of 112 bytes
$ … -d '{"wants":["3bf5d89c…"],"haves":["44b9a0f2…"]}' -o fetch2.bin     # only what the client lacks
1698 bytes
```

`POST /objects` uploads such a stream (`{"stored":n,"skipped":n}`); `GET /objects/{id}` returns the raw payload with `X-Agit-Kind: blob|tree|commit`; `GET /objects/{id}/json` decodes it:

```sh
$ curl -s -H "$H" -D - -o /dev/null $U/repos/demo/objects/3bf5d89c… | grep -i 'x-agit-kind'
x-agit-kind: commit
$ curl -s -H "$H" $U/repos/demo/objects/3bf5d89c…/json | jq -c '{kind, parents: .commit.parents, message: .commit.message}'
{"kind":"commit","parents":["44b9a0f2…","edf2c5ac…"],"message":"Merge pull request #1 from refs/heads/feature/config\n\nLoad greeting from Config"}
```

This is exactly what `agit push` (missing, then objects, then refs/update) and `agit fetch` (fetch stream) do. The TypeScript SDK ships `encodeObjectStream`/`decodeObjectStream` but no BLAKE3, so building commits from a browser is not possible; use the CLI for that.

## Browsing

Read-only endpoints the web UI is built on. Revisions are ids, branch names, tags or `HEAD`; a branch name containing `/` must be URL-encoded in a path segment (`commits/feature%2Fenv`), and the server does not understand `~`/`^`.

```sh
$ curl -s -H "$H" "$U/repos/demo/tree/main?path=src" | jq -c '{kind, entries: [.entries[] | {name, mode, size}]}'
{"kind":"tree","entries":[{"name":"config.rs","mode":"file","size":126},{"name":"lib.rs","mode":"file","size":19},{"name":"main.rs","mode":"file","size":116}]}
$ curl -s -H "$H" "$U/repos/demo/commits?rev=main&limit=3&agent=coder-1" | jq -c '.[] | {id: .id[0:12], message, task}'
{"id":"edf2c5ac6175","message":"Load the greeting from Config","task":"DEMO-7"}
{"id":"83e741ad27e1","message":"Punctuation","task":null}
$ curl -s -H "$H" $U/repos/demo/commits/main | jq -c '{changes: [.changes[].path], notes: [.notes[].title], signature}'
{"changes":["src/config.rs","src/main.rs"],"notes":["PR #1: greeting moved into Config","Greeting text lives in Config, not main.rs","Config::load() is a stub"],"signature":{"status":"unsigned","trusted":false}}
$ curl -s -H "$H" "$U/repos/demo/diff?from=44b9a0f2…&to=main" | jq -c '{stats, files: [.changes[].path]}'
{"stats":[{"path":"src/config.rs","added":5,"removed":0},{"path":"src/main.rs","added":4,"removed":1}],"files":["src/config.rs","src/main.rs"]}
$ curl -s -H "$H" "$U/repos/demo/blame/main?path=src/main.rs" | jq -c '{truncated, lines: [.lines[] | {line, commit: .commit[0:12]}]}'
{"truncated":false,"lines":[{"line":1,"commit":"3bf5d89c19f4"},{"line":2,"commit":"3bf5d89c19f4"},{"line":3,"commit":"f61ab45c8fb6"},…]}
```

`GET /commits` returns flat entries (`{id, …commit fields}`); `GET /commits/{rev}` returns `{id, commit, changes, patch, notes, signature}` where `notes` holds up to 10 knowledge notes linking the commit or covering its changed paths, and `signature` is `{status: unsigned|valid|invalid, key_id?, signer?, public_key?, reason?, trusted}`. `GET /tree/{rev}?path=` returns a directory (`entries` with sizes, no blob reads) or a file (`{kind:"blob", id, size, binary, text?}`). `GET /agents` summarises activity:

```sh
$ curl -s -H "$H" $U/repos/demo/agents
[{"id":"reviewer-1","commits":1,"last_seen_ms":…,"open_pulls":0,"leases":0},{"id":"coder-1","model":"demo-model-1","commits":2,"last_seen_ms":…,"open_pulls":0,"leases":0}]
```

## Events and SSE

```sh
$ curl -s -H "$H" "$U/repos/demo/events?since=0&limit=5" | jq -c '.[] | {seq, kind, actor}'
{"seq":35,"kind":"note.draft","actor":"reviewer-1"}
{"seq":36,"kind":"note.create","actor":"coder-1"}
…
```

`GET /events?since=N&limit=M` returns the **last** `M` events with `seq > N`. The stream endpoint replays everything after `since` and then stays live; each message is a plain `data:` line with the event JSON and `id:` set to the sequence number (no `event:` name, so `EventSource.onmessage` works):

```sh
$ curl -s -N --max-time 2 -H "$H" "$U/repos/demo/events/stream?since=0" | head -n 6
id: 1
data: {"seq":1,"time_ms":1792092720609,"kind":"repo.create","data":{"name":"demo"}}

id: 2
data: {"seq":2,"time_ms":1792092720615,"kind":"ref.update","actor":"anonymous","data":{"ref":"refs/heads/main","old":null,"new":"44b9a0f2…","message":null}}
```

Without `since` only live events are sent. Behind a proxy, disable response buffering for this path ([13](13-server-administration.md#reverse-proxy)).

## Leases (server-wide)

Same model as local leases, but shared by every client of the repository:

```sh
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/leases \
    -d '{"holder":"coder-3","paths":["docs/**"],"mode":"exclusive","ttl_secs":600,"note":"docs sweep"}'
{"id":"7967d2ef828e","holder":"coder-3","paths":["docs/**"],"mode":"exclusive","created_ms":…,"expires_ms":…,"note":"docs sweep"}
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/leases/check -d '{"holder":"coder-1","paths":["docs/intro.md"]}'
{"ok":false,"error":{"code":"lease_conflict","message":"lease conflict: docs/intro.md is held by coder-3 (lease 7967d2ef828e)","retryable":false,"details":{"path":"docs/intro.md","holder":"coder-3","lease_id":"7967d2ef828e"}}}
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/leases/7967d2ef828e/renew -d '{"ttl_secs":1200}' | jq -c '{id, expires_ms}'
$ curl -s -X DELETE -H "$H" $U/repos/demo/leases/7967d2ef828e
{"released":"7967d2ef828e"}
```

Server leases are not enforced by the server on pushes; they are coordination data for agents, the `context` endpoint, the Claude Code pre-edit hook and the lease board.

## Pull request lifecycle

The PR object (API.md "Pull requests") has `number, title, body, intent, task, source, target, source_id, target_id, base_id, author{id, model}, state, created_ms, updated_ms, merged_commit, reviews[], comments[], checks[]`. The lifecycle, as run in [chapter 4](04-quickstart-agents.md):

1. `POST /pulls {title, body?, intent?, task?, source, target?}` opens it (event `pull.open`); `agit pr create --push` pushes first.
2. `GET /pulls/{n}` returns `{pr, commits, changes, mergeable, conflicts}`; `GET /pulls/{n}/diff` the patch.
3. `POST /pulls/{n}/comments {path?, line?, kind, body}` adds a typed comment; `POST /pulls/{n}/comments/{id}/resolve` resolves it (there is no CLI form for resolve).
4. `POST /pulls/{n}/reviews {decision, body}` with `approve | request_changes | comment`; only the latest decision per reviewer counts.
5. `POST /pulls/{n}/checks {name, status, url?}` with `pending | success | failure`, typically from a CI agent.
6. `POST /pulls/{n}/merge {strategy: "merge"|"ff", message?}` evaluates the policy, performs the merge server-side and moves the target with a CAS: `{pr, merged_commit}`, or 409 `merge_conflicts`, 403 `policy_violation`, 409 `ref_conflict`. A successful merge also creates a draft knowledge note (`note.draft`).
7. `POST /pulls/{n}/close`; `POST /pulls/{n}/update {title?, body?, intent?, task?}`.

```sh
$ curl -s -X POST -H "$H" -H 'X-Agit-Agent: reviewer-1' $U/repos/demo/pulls/1/comments/1/resolve | jq -c '.comments'
[{"id":1,"author":"reviewer-1","path":"src/config.rs","line":4,"kind":"blocker","body":"Hard-coded greeting; read it from the environment.","resolved":true,"time_ms":…}]
$ curl -s -X POST -H "$H" -H 'X-Agit-Agent: ci-bot' -H 'content-type: application/json' $U/repos/demo/pulls/1/checks -d '{"name":"tests","status":"success","url":"https://ci.example/run/42"}' | jq -c '.checks'
[{"name":"tests","status":"success","url":"https://ci.example/run/42","time_ms":…}]
$ curl -s -H "$H" "$U/repos/demo/pulls?state=merged" | jq -c '.[] | {number, state, merged_commit}'
{"number":1,"state":"merged","merged_commit":"3bf5d89c…"}
```

## Policy

`GET /policy` returns the policy (defaults when unset); `PUT /policy` replaces it (admin). Fields and rules are in [5](05-concepts.md#policies).

```sh
$ curl -s -X PUT -H "$H" -H 'content-type: application/json' $U/repos/demo/policy \
    -d '{"protected_branches":["main"],"require_pull_request":true,"require_approvals":1,"require_intent":true,"require_agent":true}'
{"protected_branches":["main"],"require_pull_request":true,"require_approvals":1,"require_intent":true,"require_agent":true,"require_checks":false,"block_on_blockers":true,"allowed_merge_agents":[],"allowed_push_agents":[],"max_files_per_commit":0,"require_signed":false}
```

Omitted fields take their defaults (`block_on_blockers` defaults to `true`). Violations:

```json
{"ok":false,"error":{"code":"policy_violation","message":"policy violation: refs/heads/main is protected; changes must go through a pull request","retryable":false}}
{"ok":false,"error":{"code":"policy_violation","message":"policy violation: policy violation: a reviewer requested changes","retryable":false}}
{"ok":false,"error":{"code":"policy_violation","message":"policy violation: policy violation: commit f7d8f7966e49 is not signed (require_signed)","retryable":false}}
```

(The doubled prefix on merge violations is cosmetic.)

## Signing keys

```sh
$ curl -s -H "$H" $U/repos/demo/keys
[{"agent":"coder-1","key_id":"0ae983c0881ef763","public_key":"da2e81ef…","added_ms":…,"added_by":"coder-1"}]
$ curl -s -H "$H" $U/repos/demo/commits/feature%2Fenv | jq -c .signature
{"status":"valid","key_id":"0ae983c0881ef763","signer":"coder-1","public_key":"da2e81ef…","trusted":true}
```

`POST /keys {agent, public_key}` registers (409 when the key belongs to another agent; idempotent for the same agent), `DELETE /keys/{key_id}` removes. `agit key generate --register` and `agit key register` call `POST /keys`. `trusted` is `true` only when the signer's key is registered.

## Webhooks

```sh
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/webhooks \
    -d '{"url":"http://127.0.0.1:17980/agit","kinds":["pull.*","note.*","ping"],"secret":"s3cret"}'
{"id":"wh_6bb0ea4371","url":"http://127.0.0.1:17980/agit","kinds":["pull.*","note.*","ping"],"active":true,"has_secret":true,"created_ms":…}
$ curl -s -X POST -H "$H" $U/repos/demo/webhooks/wh_6bb0ea4371/test
{"queued":true}
```

What the receiver saw (a 10-line Python HTTP server logging headers and body):

```json
{"headers":{"X-Agit-Event":"ping","X-Agit-Repo":"demo","X-Agit-Delivery":"d23bc55d89bc402087bc725a87a09d80","X-Agit-Webhook":"wh_6bb0ea4371","X-Agit-Signature":"blake3=939d73d8…"},"body":{"repo":"demo","event":{"seq":0,"kind":"ping",…}}}
{"headers":{"X-Agit-Event":"pull.open",…,"X-Agit-Signature":"blake3=8348bf55…"},"body":{"repo":"demo","event":{"seq":69,"kind":"pull.open","actor":"coder-1",…}}}
```

Kind filters accept exact kinds, `prefix.*`, `prefix.` and `*`; an empty list means everything. The signature is `blake3::keyed_hash(derive_key("agit webhook v1", secret), body)` over the raw body; verify it with a constant-time compare and use `X-Agit-Delivery` as an idempotency key, because deliveries retry three times with backoff (200 ms, 800 ms, 3.2 s) on non-2xx responses or connection errors. The secret is stored in plaintext in `webhooks.json` and never returned by the API.

## Knowledge notes

The note object and every query parameter are in API.md "Knowledge notes"; the behaviour is explained in [11](11-knowledge-base.md). The calls used in this handbook:

```sh
# tag rules (admin)
$ curl -s -X PUT -H "$H" -H 'content-type: application/json' $U/repos/demo/knowledge/rules \
    -d '[{"pattern":"src/**","tags":["area:core","lang:rust"]},{"pattern":"vendor/**","tags":["area:vendor"]}]'
# ranked search with a path filter
$ curl -s -H "$H" "$U/repos/demo/knowledge/notes?q=environment+greeting" | jq -c '.[] | {title, score, stale}'
{"title":"Config::load() reads GREETING from the environment","score":1.2596087,"stale":false}
{"title":"PR #1: greeting moved into Config","score":0.1753074,"stale":false}
# drafts left by a merge
$ curl -s -H "$H" "$U/repos/demo/knowledge/notes?drafts=1&pull=1" | jq -c '.[] | {id, title, draft, tags, paths, confidence}'
{"id":"n_599ca51c9760","title":"Load greeting from Config","draft":true,"tags":["kind:change","area:core","lang:rust"],"paths":["src/**"],"confidence":0.4}
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/knowledge/notes/n_599ca51c9760/accept -d '{"title":"PR #1: greeting moved into Config","confidence":0.7}'
# staleness
$ curl -s -H "$H" $U/repos/demo/knowledge/stale | jq -c '.[] | {id, title, changed_paths}'
{"id":"n_1f5d3dce7c3b","title":"Config::load() is a stub","changed_paths":["src/config.rs"]}
$ curl -s -X PATCH -H "$H" -H 'content-type: application/json' $U/repos/demo/knowledge/notes/n_1f5d3dce7c3b -d '{"title":"Config::load() reads GREETING from the environment","body":"…"}'
$ curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/knowledge/notes/n_1f5d3dce7c3b/validate -d '{}' | jq -c '{stale, valid_at: .valid_at[0:12]}'
{"stale":false,"valid_at":"b1b1e19abf97"}
$ curl -s -H "$H" $U/repos/demo/knowledge/tags
[{"tag":"area:core","count":3},{"tag":"lang:rust","count":2},{"tag":"kind:change","count":1},{"tag":"kind:decision","count":1},{"tag":"kind:gotcha","count":1}]
```

Create is `POST /knowledge/notes {title, body?, tags?, paths?, commits?, pulls?, confidence?, valid_at?, draft?}` (201), update is `PATCH`, delete is `DELETE` (deleting a draft discards it). `valid_at` must be an object the server has (412 otherwise).

## Context packs

```sh
$ curl -s -H "$H" "$U/repos/demo/context?path=src/config.rs&include=files&budget=400"
{"path":"src/config.rs","notes":[{"id":"n_599ca51c9760","title":"PR #1: greeting moved into Config","tags":["kind:change","area:core","lang:rust"],"paths":["src/**"],"author":"reviewer-1","confidence":0.7,"updated_ms":…,"excerpt":"Intent: Make the greeting configurable for DEMO-7\nTask: DEMO-7\n\n…","valid_at":"3bf5d89c…","draft":false,"stale":false},…],"suggested_tags":["area:core","lang:rust"],"recent_commits":[…],"leases":[],"open_pulls":[],"agents_active":["coder-1","reviewer-1"],"files":[…]}
```

`limit` (default 10) caps notes and recent commits. `include=files` adds the file at `path` and up to five siblings, most recently touched first, trimmed so the whole response stays within about `budget * 4` characters (`budget` is approximate tokens, default 6000); with a budget as small as 400 the files list comes back empty. Note bodies are never included, only 200-character excerpts; fetch a note with `GET /knowledge/notes/{id}` when the excerpt is not enough.

## What is not in the API yet

Idempotency keys on mutating calls, WebSocket streaming, gRPC/HTTP-2 pack streaming, organisations and users, cross-repository PRs, note objects in the repository, and a `version` field per response are all 📋 ([docs/ROADMAP.md](../ROADMAP.md), RFCs 0004 to 0007).
