# agit server HTTP API v1

Base path: `/api/v1`. JSON unless stated otherwise. All ids are 64-char hex
BLAKE3. Every error body is `{"ok":false,"error":{"code","message","retryable"}}`
with the same codes as the CLI (`ref_conflict`, `object_not_found`,
`lease_conflict`, `not_found`, `forbidden`, `invalid`, ...). Success bodies are
plain JSON objects (no envelope) unless noted.

Headers:
- `Authorization: Bearer <token>` required when the server runs with a token.
- `X-Agit-Agent: <agent id>` identifies the acting agent for events, leases and PRs.

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
- `POST /repos/{repo}/objects` body: object stream -> `{"stored":n,"skipped":n}`
- `GET /repos/{repo}/objects/{id}` -> raw payload, header `X-Agit-Kind: blob|tree|commit`
- `GET /repos/{repo}/objects/{id}/json` -> `{id, kind, size, tree?|commit?|text?}`
- `POST /repos/{repo}/fetch` `{"wants":[ids],"haves":[ids]}` -> object stream of everything
  reachable from wants and not from haves (commits, trees, blobs).

## Browsing (read-only, used by the web UI)
- `GET /repos/{repo}/commits?rev=main&limit=50&agent=&task=&path=` -> `[{id, ...commit}]`
- `GET /repos/{repo}/commits/{rev}` -> `{id, commit, changes:[...], patch}`
- `GET /repos/{repo}/tree/{rev}?path=src/x` -> directory: `{kind:"tree", path, entries:[{name,mode,id,size}]}`
  or file: `{kind:"blob", path, id, size, binary, text?}`
- `GET /repos/{repo}/diff?from=<rev>&to=<rev>` -> `{changes, patch, stats}`
- `GET /repos/{repo}/branches` -> `[{name,id,ahead?,behind?}]`, `GET /repos/{repo}/tags`

## Events
- `GET /repos/{repo}/events?since=<seq>&limit=` -> `[event]`
- `GET /repos/{repo}/events/stream?since=<seq>` -> `text/event-stream`, each `data:` is an event JSON.
Event kinds mirror the CLI plus `ref.update`, `pull.open`, `pull.review`, `pull.merge`, `pull.close`, `note.create`.

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

## Knowledge notes
Note object:
```json
{"id":"n_...","title":"...","body":"markdown","tags":["area:auth","kind:gotcha"],
 "paths":["src/auth/**"],"commits":["<id>"],"pulls":[3],
 "author":"agent1","confidence":0.9,"created_ms":0,"updated_ms":0,"superseded_by":null}
```
- `GET /repos/{repo}/knowledge/notes?tags=a,b&path=src/auth/login.rs&q=text&limit=` -> `[note summary]`
  (summary = note without body, plus `excerpt`). `tags` means all tags must match. `path` matches notes whose `paths` cover it.
- `GET /repos/{repo}/knowledge/notes/{id}` -> note
- `POST /repos/{repo}/knowledge/notes` -> note
- `PATCH /repos/{repo}/knowledge/notes/{id}` -> note
- `DELETE /repos/{repo}/knowledge/notes/{id}`
- `GET /repos/{repo}/knowledge/tags` -> `[{tag, count}]`
- `GET /repos/{repo}/knowledge/rules` / `PUT` -> `[{pattern:"src/auth/**", tags:["area:auth"]}]`
  Tag rules tell agents which tags to attach when writing notes about a path.
- `GET /repos/{repo}/context?path=src/auth/login.rs&limit=` -> `{notes:[summary], recent_commits:[...], leases:[...], open_pulls:[...]}`
  One call that returns what an agent should read before touching a path.

## Activity
- `GET /repos/{repo}/agents` -> `[{id, commits, last_seen_ms, open_pulls, leases}]` derived from commits and events.
