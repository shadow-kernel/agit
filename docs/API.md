# agit server HTTP API v1

Base path: `/api/v1`. JSON unless stated otherwise. All ids are 64-char hex
BLAKE3. Every error body is `{"ok":false,"error":{"code","message","retryable"}}`
with the same codes as the CLI (`ref_conflict`, `object_not_found`,
`lease_conflict`, `not_found`, `forbidden`, `invalid`, ...). Success bodies are
plain JSON objects (no envelope) unless noted.

Headers:
- `Authorization: Bearer <token>`: a session token from an SSH-key sign-in (`POST /auth/session`), a scoped token from `POST /auth/tokens`, or the root token (`AGIT_SERVER_TOKEN`). Required on every request as soon as the server is *closed*, which it is whenever a root token or an admin key (`--admin-key`) is configured. Without either the server runs in open mode (every request acts as an anonymous root) for local development; `agit-server` refuses to start open on a non-loopback address unless `--allow-open` is given.
- `X-Agit-Agent: <agent id>` identifies the acting agent for events, leases and PRs. A token or key bound to an agent overrides this header, so bound identities cannot be spoofed.

Two error codes: `401 unauthorized` means no usable credentials (missing, invalid, expired or revoked token; the response carries `WWW-Authenticate: Bearer realm="agit"`), `403 forbidden` means the credentials are valid but lack the scope the request needs. Clients should sign in on 401 and give up on 403.

## Auth

Scopes are `{repo: "*" | <name>, access: "read" | "write" | "admin"}`. Access needed per request: `GET` read; other methods write; repository create/delete, `policy`, `keys`, `webhooks`, `knowledge/rules`, `/auth/tokens` and `/auth/keys` admin. Repository listings are filtered to the repositories a principal can read. A principal can only grant scopes it holds: a repository scope needs admin on that repository, a `*` scope needs root or a `*:admin` scope. Denied requests and every mutation are appended to `<data-dir>/audit.jsonl`.

SSH keys (the way people sign in; `agit auth login` does all of this):
- `POST /auth/challenge` (public, no body) -> `{nonce, expires_ms}`. The nonce is single-use and valid for two minutes.
- `POST /auth/session` (public) `{public_key, nonce, signature, ttl_secs?}` -> `201` `{token, id, name, agent?, scopes, created_ms, expires_ms, created_by, key}`. `public_key` is the `ssh-ed25519 …` line (comment optional) or a 64-hex Ed25519 key; `signature` is the hex Ed25519 signature over the bytes `agit-auth-v1\n<nonce>`. The key must be an authorized key (`/auth/keys`) or a startup admin key (`--admin-key`, which grants `*:admin` and reports `key: "admin"`); otherwise `403 forbidden` with a message containing `not authorized`. A reused or expired nonce and a wrong signature are also `403`. The session token carries the key's scopes and bound agent; `ttl_secs` defaults to 8 hours and is capped at 30 days.
- `POST /auth/logout` (any authenticated token) -> `{revoked: <token id>}`: revokes the token the caller presented.
- `GET /auth/keys` (admin) -> `[{id, name, fingerprint, key_id, public_key, agent?, scopes, created_ms, created_by, last_used_ms?}]`. `fingerprint` is `SHA256:<base64>` as `ssh-keygen -lf` prints it; `key_id` is the short id also used for commit-signing keys. Startup admin keys are not listed.
- `POST /auth/keys` (admin) `{public_key, name?, agent?, scopes}` -> `201` key. `name` defaults to the key's comment, then its fingerprint. Only `ssh-ed25519` keys are accepted (`ssh-keygen -t ed25519`); the same key twice is `409 already_exists`.
- `DELETE /auth/keys/{id}` (admin) -> `{deleted, revoked_sessions}`: removes the key and every session it opened.

Tokens (for agents, CI and the browser):
- `GET /auth/whoami` -> `{principal, agent?, root, scopes, key?}`; `key` is set for sessions opened with an SSH key.
- `POST /auth/tokens` (admin) `{name, agent?, scopes, ttl_secs?}` -> `201` `{token, id, name, agent?, scopes, created_ms, expires_ms?, created_by, key?}`; the plaintext token is returned once.
- `GET /auth/tokens` (admin) -> `[token info]` (hashes only; `key` names the authorized key for session tokens)
- `DELETE /auth/tokens/{id}` (admin)

## Health
- `GET /health` -> `{"status":"ok","version":"0.1.0"}`

## Repositories
- `GET /repos` -> `[{name, default_branch, head, created_ms}]`
- `POST /repos` `{name, default_branch?}` -> repo summary (409 `already_exists`)
- `GET /repos/{repo}` -> `{name, default_branch, head, branches, tags, objects, created_ms}`
- `DELETE /repos/{repo}` -> `{deleted:true}`

## Refs
- `GET /repos/{repo}/refs?prefix=refs/heads/` -> `[{name,id}]`
- `POST /repos/{repo}/refs/update`
  ```json
  {"updates":[{"name":"refs/heads/main","expect":{"is":"<id>"},"new":"<id>"}],
   "message":"optional reason"}
  ```
  `expect` is `{"any":true}`, `{"absent":true}` or `{"is":"<id>"}`; `new` null deletes.
  Atomic across all updates. 409 `ref_conflict` with `{name, expected, actual}` in `details`.
  Every object referenced by a new value must already exist (412 `object_not_found`).

## Objects
Object stream format (content type `application/x-agit-objects`):
repeated frames `kind:u8 | len:u32 big-endian | payload`. kind 1 blob, 2 tree, 3 commit.
- `POST /repos/{repo}/objects/missing` `{"ids":[...]}` -> `{"missing":[...]}`
- `POST /repos/{repo}/objects` body: object stream -> `{"stored":n,"skipped":n}`. Clients send batches of about 8 MiB; an object larger than that goes in a request of its own. The body limit is `agit-server --max-upload-mb` (default 1024 MiB); larger bodies are refused with `413` (the server may close the connection while the client is still sending). Every other route accepts JSON bodies up to 2 MiB.
- `GET /repos/{repo}/objects/{id}` -> raw payload, header `X-Agit-Kind: blob|tree|commit`
- `GET /repos/{repo}/objects/{id}/json` -> `{id, kind, size, tree?|commit?|text?}`
- `POST /repos/{repo}/fetch` `{"wants":[ids],"haves":[ids]}` -> object stream of everything
  reachable from wants and not from haves (commits, trees, blobs).

## Browsing (read-only, used by the web UI)
- `GET /repos/{repo}/commits?rev=main&limit=50&agent=&task=&path=` -> `[{id, ...commit}]`
- `GET /repos/{repo}/commits/{rev}` -> `{id, commit, changes:[...], patch, notes:[note summary]}`
  `notes` holds up to 10 knowledge notes: those linking the commit in `commits` (drafts included) first, then accepted notes whose `paths` cover a changed path.
- `GET /repos/{repo}/tree/{rev}?path=src/x` -> directory: `{kind:"tree", path, entries:[{name,mode,id,size}]}`
  or file: `{kind:"blob", path, id, size, binary, text?}`
- `GET /repos/{repo}/diff?from=<rev>&to=<rev>` -> `{changes, patch, stats}`
- `GET /repos/{repo}/blame/{rev}?path=&max_commits=500` -> `{path, rev, lines:[{line, commit, text}], commits:{<id>:{id, author, agent?, time_ms, summary, intent?}}, truncated}`
- Change objects may carry `kind: "renamed"` with `from` and `similarity` when rename detection is applied (CLI `diff`/`show`).
- `GET /repos/{repo}/branches` -> `[{name,id,ahead?,behind?}]`, `GET /repos/{repo}/tags`

## Events
- `GET /repos/{repo}/events?since=<seq>&limit=` -> `[event]`
- `GET /repos/{repo}/events/stream?since=<seq>` -> `text/event-stream`; each message is a plain `data:` line with the event JSON (`id:` is the seq; no `event:` name, so `EventSource.onmessage` works). Without `since` only live events are sent; with `since=N` events with `seq > N` are replayed first.
Event kinds mirror the CLI plus `ref.update`, `pull.open`, `pull.review`, `pull.merge`, `pull.close`, `note.create`, `note.update`, `note.delete`, `note.validate`, `note.draft`, `note.accept`, `knowledge.rules`.

## Leases (server-wide coordination between agents)
- `GET /repos/{repo}/leases` -> `[lease]`
- `POST /repos/{repo}/leases` `{holder, paths:[..], mode:"exclusive"|"shared", ttl_secs, note?}` -> lease (409 `lease_conflict`)
- `POST /repos/{repo}/leases/{id}/renew` `{ttl_secs}`
- `DELETE /repos/{repo}/leases/{id}`
- `POST /repos/{repo}/leases/check` `{holder, paths}` -> `{allowed:true}` or 409

## Pull requests
PR object:
```json
{"number":1,"title":"...","body":"...","intent":"...","task":"T-1",
 "source":"refs/heads/ws/agent1","target":"refs/heads/main",
 "source_id":"<id>","target_id":"<id>","base_id":"<id>",
 "author":{"id":"agent1","model":null},"state":"open|merged|closed",
 "created_ms":0,"updated_ms":0,"merged_commit":null,
 "reviews":[{"reviewer":"agent2","decision":"approve|request_changes|comment","body":"","time_ms":0}],
 "comments":[{"id":1,"author":"","path":"src/x.rs","line":12,"kind":"question|suggestion|blocker|note","body":"","resolved":false,"time_ms":0}],
 "checks":[{"name":"tests","status":"pending|success|failure","url":null,"time_ms":0}]}
```
- `GET /repos/{repo}/pulls?state=open` -> `[pr]`
- `POST /repos/{repo}/pulls` `{title, body?, intent?, task?, source, target?}` -> pr
- `GET /repos/{repo}/pulls/{n}` -> `{pr, commits:[...], changes:[...], mergeable:true|false, conflicts:[...]}`
- `GET /repos/{repo}/pulls/{n}/diff` -> `{changes, patch, stats}`
- `POST /repos/{repo}/pulls/{n}/comments` `{path?, line?, kind, body}` -> comment
- `POST /repos/{repo}/pulls/{n}/comments/{id}/resolve`
- `POST /repos/{repo}/pulls/{n}/reviews` `{decision, body}` -> pr
- `POST /repos/{repo}/pulls/{n}/checks` `{name, status, url?}` -> pr
- `POST /repos/{repo}/pulls/{n}/merge` `{strategy:"merge"|"ff", message?}` -> `{pr, merged_commit}` (409 `merge_conflicts`)
- `POST /repos/{repo}/pulls/{n}/close` -> pr
- `POST /repos/{repo}/pulls/{n}/update` `{title?, body?, intent?, task?}` -> pr

## Policy
Per-repository rules enforced at `refs/update` (direct pushes) and `pulls/{n}/merge`. Violations are `403 policy_violation`.
```json
{"protected_branches":["main","release/*"],"require_pull_request":true,"require_approvals":1,
 "require_intent":true,"require_agent":false,"require_checks":false,"block_on_blockers":true,
 "allowed_merge_agents":[],"allowed_push_agents":[],"max_files_per_commit":0,"require_signed":false}
```
- `GET /repos/{repo}/policy` -> policy (defaults when unset)
- `PUT /repos/{repo}/policy` -> policy
Rules: protected branches cannot be deleted; with `require_pull_request` they only move through PR merges; approvals count the latest decision per reviewer and exclude the PR author; any `request_changes` blocks; unresolved `blocker` comments block when `block_on_blockers`; `require_intent`/`require_agent`/`max_files_per_commit` are checked on every new commit the update introduces.

## Signing keys
Agents sign commits with Ed25519 (`agit key generate`, `agit commit --sign`). The public key travels inside the commit; the registry decides which keys are trusted for which agent. `GET /repos/{repo}/commits/{rev}` returns `signature: {status: unsigned|valid|invalid, key_id?, signer?, public_key?, reason?, trusted}`.
- `GET /repos/{repo}/keys` -> `[{agent, key_id, public_key, added_ms, added_by}]`
- `POST /repos/{repo}/keys` `{agent, public_key}` -> key (409 when the key belongs to another agent)
- `DELETE /repos/{repo}/keys/{key_id}`
Policy `require_signed` makes every commit reaching a protected branch need a valid signature with a key registered for its signer.

## Webhooks
Deliver events to HTTP endpoints. Payload `{"repo":"<name>","event":<event>}`, headers `X-Agit-Event` (kind), `X-Agit-Repo`, `X-Agit-Delivery` (unique id), `X-Agit-Webhook` (id) and, when a secret is set, `X-Agit-Signature: blake3=<hex>` where the value is `blake3::keyed_hash(derive_key("agit webhook v1", secret), body)`. Deliveries retry 3 times with backoff.
- `GET /repos/{repo}/webhooks` -> `[{id, url, kinds, active, has_secret, created_ms}]`
- `POST /repos/{repo}/webhooks` `{url, kinds?: ["commit","pull.*"], secret?}` -> webhook
- `DELETE /repos/{repo}/webhooks/{id}`
- `POST /repos/{repo}/webhooks/{id}/test` -> queues a `ping` event

## Knowledge notes
Clients may keep a local mirror in `.agit/knowledge` (`agit know --local`, offline fallback); local-only notes carry `n_local_` ids until `agit know sync` uploads them and caches the server's notes.
Note object:
```json
{"id":"n_...","title":"...","body":"markdown","tags":["area:auth","kind:gotcha"],
 "paths":["src/auth/**"],"commits":["<id>"],"pulls":[3],
 "author":"agent1","confidence":0.9,"created_ms":0,"updated_ms":0,"superseded_by":null,
 "valid_at":"<id>","draft":false,"stale":false}
```
- `valid_at`: commit the note was written or last validated against. Defaults to HEAD of the default branch at creation (when one exists); a given value must be an object the server has (412 `object_not_found`).
- `stale` (computed on every read, never stored): `true` when any path covered by `paths` differs between `valid_at` and HEAD. Notes without `valid_at` or without `paths` are never stale. The per-commit-pair diff is cached in memory.
- `draft`: machine-generated note (see "Drafts from merges"). Drafts are hidden from listings, context and tag counts until accepted. The note object omits `draft` when false; summaries always carry it.

Summary = note without `body`, plus `excerpt` (first 200 chars), `valid_at`, `draft`, `stale`, and `score` (BM25 relevance, only present for `q=` queries).

- `GET /repos/{repo}/knowledge/notes?tags=a,b&path=src/auth/login.rs&q=text&commit=<rev>&pull=<n>&drafts=1&include_superseded=1&limit=` -> `[note summary]`
  - `q`: full-text search over title, body, tags and paths with BM25 ranking (title weight 3, tags 2, body and paths 1). Tokens are lowercase unicode alphanumeric runs; camelCase and snake_case words are indexed both whole and split (`SessionStore` -> `sessionstore`, `session`, `store`); `namespace:value` tags are indexed whole as well as split. Query terms match exactly, and terms of 3+ characters also match as prefixes at half weight. Results come best first with `score` set; without `q` results are ordered by `updated_ms` descending and `score` is absent. The index is built per repository in memory at the first query and kept current on create/patch/delete.
  - `tags`: all listed tags must be present. `path`: notes whose `paths` cover it. `commit`: notes listing that commit (any revision syntax). `pull`: notes listing that PR number. All of these are post-filters applied in rank order.
  - `drafts=1` includes drafts, `include_superseded=1` includes superseded notes. `limit` defaults to 100.
- `GET /repos/{repo}/knowledge/notes/{id}` -> note (with `stale`)
- `POST /repos/{repo}/knowledge/notes` `{title, body?, tags?, paths?, commits?, pulls?, confidence?, valid_at?, draft?}` -> note (201). Emits `note.create`.
- `PATCH /repos/{repo}/knowledge/notes/{id}` `{title?, body?, tags?, paths?, commits?, pulls?, confidence?, superseded_by?}` -> note. Emits `note.update`.
- `DELETE /repos/{repo}/knowledge/notes/{id}` -> `{deleted: id}`. Emits `note.delete`. Deleting a draft discards it.
- `POST /repos/{repo}/knowledge/notes/{id}/validate` `{rev?}` -> note. Sets `valid_at` to `rev` (default HEAD) and bumps `updated_ms`: the caller confirms the note still holds there. Emits `note.validate`.
- `POST /repos/{repo}/knowledge/notes/{id}/accept` body optional, same shape as PATCH -> note. Clears `draft`, applying the patch first when given. Emits `note.accept`.
- `GET /repos/{repo}/knowledge/stale?rev=HEAD&drafts=1` -> `[{...note summary, stale:true, changed_paths:[...]}]`
  Non-superseded notes (drafts only with `drafts=1`) whose covered paths changed between their `valid_at` and `rev`; `changed_paths` lists those paths.
- `GET /repos/{repo}/knowledge/tags` -> `[{tag, count}]` over accepted (non-draft) notes.
- `GET /repos/{repo}/knowledge/rules` / `PUT` -> `[{pattern:"src/auth/**", tags:["area:auth"]}]`
  Tag rules tell agents which tags to attach when writing notes about a path. Emits `knowledge.rules`.

### Drafts from merges
Every `POST /pulls/{n}/merge` that succeeds also creates a draft note (`draft: true`) and emits `note.draft` `{id, title, pull, commit, tags, paths}`; a failure to draft is logged and never fails the merge. The draft has: `title` = PR title; `body` = `Intent:`/`Task:` lines, the PR body, a `Changed files:` bullet list (`- path (A|M|D|T)`) and a `Reviews:` list with each reviewer's latest decision; `tags` = `kind:change` plus the tags the rules suggest for the changed paths; `paths` = the changed files' directories as `dir/**` (root-level files by name), deduplicated; `pulls` = `[n]`; `commits` = `[merged_commit]`; `author` = the merging agent; `confidence` = 0.4; `valid_at` = the merged commit. Find them with `GET /knowledge/notes?drafts=1&pull=<n>`, then `accept` (optionally editing) or `DELETE`. There is currently no switch to turn drafting off.

### Context
- `GET /repos/{repo}/context?path=src/auth/login.rs&limit=&include=files&budget=6000` ->
  `{path, notes:[summary], suggested_tags, recent_commits:[...], leases:[...], open_pulls:[...], agents_active:[...], files?:[{path, text, truncated}]}`
  One call that returns what an agent should read before touching a path.
  - `notes`: notes covering the path, fresh ones first, then stale ones (`stale: true`); drafts excluded. `limit` (default 10) caps notes and `recent_commits`.
  - `agents_active`: sorted agent ids that hold a lease overlapping the path or committed to it in the last 24 hours (agent id, or author name for commits without an agent).
  - `include=files` adds `files`: the file at `path` (when it is a file) followed by up to 5 sibling files in the same directory (or, for a directory, up to 5 of its files), most recently touched first; binary files are skipped. The whole response is kept within `budget * 4` characters (`budget` is approximate tokens, default 6000): the file texts are trimmed last-first, `truncated: true` marks a cut file, and files that do not fit at all are dropped. Note bodies are never included, only excerpts. Without `include=files` the field is omitted.

## Activity
- `GET /repos/{repo}/agents` -> `[{id, commits, last_seen_ms, open_pulls, leases}]` derived from commits and events.
