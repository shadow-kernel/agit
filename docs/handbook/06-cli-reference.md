# 6. CLI reference

`agit` 0.1.0. Every command accepts the global options below, and every command prints one JSON envelope with `--json`. Flags and defaults are copied from `agit <command> --help`; JSON shapes are copied from real `--json` output (long ids shortened in prose, always 64 hex characters on the wire).

```
Usage: agit [OPTIONS] <COMMAND>

Options:
      --json     Machine-readable JSON output (stable envelope: {ok, result|error}) [env: AGIT_JSON=]
  -C <DIR>       Run as if started in this directory
  -h, --help     Print help
  -V, --version  Print version
```

## The JSON contract

Success:

```json
{"ok": true, "result": <command-specific>}
```

Failure (also printed with `--json` when the command fails):

```json
{"ok": false, "error": {"code": "lease_conflict", "message": "lease conflict: src/main.rs is held by coder-1 (lease c618a7ee7a6c)", "retryable": false, "details": {"path": "src/main.rs", "holder": "coder-1", "lease_id": "c618a7ee7a6c"}}}
```

`details` is `null` unless the error carries structure: `ref_conflict {name, expected, actual}`, `lease_conflict {path, holder, lease_id}`, `merge_conflicts {count}`, `dirty_worktree {paths}`, `object_not_found {id}`. Streaming commands (`events --follow`) print one JSON object per line instead of an envelope. Without `--json`, errors go to stderr as `error: <message>` plus `hint: this error is transient; retry the command` when retryable.

`--json` is a boolean flag; the environment form is `AGIT_JSON=true` (`AGIT_JSON=1` is rejected by clap with "invalid value '1'").

## Exit codes and error codes

| Exit | Meaning | Codes |
| --- | --- | --- |
| 0 | success | |
| 1 | error | everything not listed below, including `policy_violation`, `nothing_to_commit`, `not_a_repository`, `invalid`, `error` |
| 3 | conflict | `merge_conflicts`, `lease_conflict`, `dirty_worktree` |
| 75 | retryable | `ref_conflict`, `lock_timeout` |

Codes produced by the core library (stable; branch on these):

| code | meaning | retryable |
| --- | --- | --- |
| `not_a_repository` | no `.agit` here or in any parent | no |
| `already_exists` | `init` in an existing repository; key registered for another agent (server) | no |
| `object_not_found` | id is not in the store (server: HTTP 412) | no |
| `corrupt_object` | object failed to decode | no |
| `hash_mismatch` | object bytes do not hash to the id | no |
| `ref_not_found` | named ref does not exist | no |
| `invalid_ref_name` | ref name violates the naming rules | no |
| `ref_conflict` | compare-and-swap failed, the ref moved | **yes** |
| `invalid_object_id` | not a 64-hex id | no |
| `ambiguous_revision` | prefix matches several objects | no |
| `unknown_revision` | revision cannot be resolved | no |
| `path_outside_worktree` | path escapes the working tree | no |
| `path_not_tracked` | path is not in the index | no |
| `nothing_to_commit` | index equals HEAD | no |
| `dirty_worktree` | operation needs a clean tree (`details.paths`) | no |
| `merge_conflicts` | merge stopped with conflicts (`details.count`) | no |
| `lease_conflict` | path under someone else's exclusive lease | no |
| `lease_not_found` | lease id unknown or expired | no |
| `workspace_exists` / `workspace_not_found` | workspace name taken / unknown | no |
| `lock_timeout` | internal lock busy for 10 s (refs) or 5 s (leases, events) | **yes** |
| `policy_violation` | server policy refused the change | no |
| `not_found` | server: repository, PR, note, key or webhook unknown | no |
| `forbidden` | server: missing/invalid token or insufficient scope | no |
| `invalid` | invalid argument or state (also client-side non-fast-forward push) | no |
| `io` | filesystem error, path in message | no |
| `serde` | (de)serialization error | no |

Errors raised by the CLI layer itself rather than the core (for example `nothing specified; use agit add`, `unknown config key foo`, `merge needs a revision`, `invalid duration unit`) have `code: "error"`, exit 1. Treat `error` as "read the message".

## Environment variables

| Variable | Used by | Meaning |
| --- | --- | --- |
| `AGIT_JSON` | all commands | `true` enables `--json` |
| `AGIT_AGENT` | commit, checkpoint, merge, pull, revert, cherry-pick, lease (holder), ws new, pr, know, key, mcp | agent id recorded in commits and used as lease holder and `X-Agit-Agent` |
| `AGIT_MODEL`, `AGIT_RUN_ID`, `AGIT_ON_BEHALF_OF` | same | the other `agent` fields (`model`, `run_id`, `on_behalf_of`) |
| `AGIT_AUTHOR_NAME`, `AGIT_AUTHOR_EMAIL` | commit author, reflog actor | fallback when `user.name`/`user.email` are not configured |
| `AGIT_TOKEN` | clone, fetch, push, pull, pr, know, key register, mcp, auth | bearer token for the server; without it the session cached by `agit auth login` is used, and a `401` triggers an automatic SSH sign-in |
| `AGIT_SSH_KEY` | auth login, automatic sign-in | private key to sign in with (default `~/.ssh/id_ed25519`, then ssh-agent, then `~/.agit/keys`) |
| `AGIT_AUTO_LOGIN` | remote commands | `0` disables the automatic SSH sign-in on `401` |
| `AGIT_SESSIONS_FILE` | auth | session cache (default `~/.agit/sessions.json`) |
| `AGIT_SERVER_URL` | auth | server for `agit auth` outside a repository |
| `AGIT_SIGN` | commit, merge | `auto` (default): sign when a key named after the agent exists; `1|on|true|yes`: always; `0|off|false|no`: never |
| `AGIT_KEY` | commit `--sign`, key commands | default key name (else `AGIT_AGENT`, else `default`) |
| `AGIT_KEYS_DIR` | key commands, signing | key directory (default `~/.agit/keys`) |
| `AGIT_FSYNC` | every write | `refs` (default), `full`, `all`, `none` ([5](05-concepts.md#pack-segments-and-durability)) |
| `AGIT_PACK` | object store | `0|off|false` writes loose objects instead of pack segments |
| `AGIT_LEASE_SYNC` | lease commands, commit, merge, hooks, MCP | `auto` (default): leases are shared through the server, local fallback while it is unreachable; `required`: an unreachable server is an error; `off`: leases stay on this machine |
| `AGIT_LEASE_REMOTE` | same | remote the leases are shared through (default `origin`) |

Server-side variables (`AGIT_DATA_DIR`, `AGIT_LISTEN`, `AGIT_SERVER_TOKEN`, `AGIT_WEB_DIR`, `RUST_LOG`) are in [13](13-server-administration.md). The Claude Code hooks additionally read `AGIT_BIN`, `AGIT_HOOKS_DISABLE` and `AGIT_HOOK_LEASE_TTL` ([7](07-mcp-reference.md#claude-code-integration)).

Agent flags (`--agent`, `--model`, `--run-id`, `--on-behalf-of`) are accepted by every command that can write a commit or talk to the server; they are listed once here and not repeated below.

Durations (`--ttl`, `--reflog-expire`, `--checkpoint-expire`) take an integer with a unit: `s`/`sec`, `m`/`min`, `h`/`hr`, `d`; a bare number is seconds; `0` means "keep all" where expiry is concerned.

---

## Local commands

### `agit init [PATH]`

Create an empty repository (default branch `main`, format version 1). Fails with `already_exists` when `.agit` exists.

```json
{"ok":true,"result":{"path":"/…/demo","common_dir":"/…/demo/.agit"}}
```

### `agit config [KEY] [VALUE]`

Keys: `user.name`, `user.email`, `default_branch`. No arguments prints the whole configuration (including `remotes`); a key alone reads it; key and value set it.

```json
{"ok":true,"result":{"format_version":1,"default_branch":"main","user":{"name":"Dana Developer","email":"dana@example.com"},"remotes":{"origin":{"url":"http://127.0.0.1:17979/api/v1/repos/demo"}}}}
{"ok":true,"result":{"key":"user.name","value":"Dana Developer"}}
```

### `agit root`

Repository locations and state; works on an unborn branch (`head: null`).

```json
{"ok":true,"result":{"work_dir":"/…/demo","common_dir":"/…/demo/.agit","workspace":"main","branch":"main","head":"f61ab45c…","merge_in_progress":false}}
```

### `agit add [PATHS]... [-A|--all] [-f|--force]`

Stage files, directories or everything (`-A`). `--force` adds ignored files. `.agit` is always ignored; `.agitignore` and `.gitignore` are honoured with gitignore semantics. Registered submodules under the given paths are staged at their nested HEAD.

```json
{"ok":true,"result":{"added":["src/config.rs"],"updated":["src/main.rs"],"removed":[],"unchanged":3}}
```

### `agit rm [PATHS]... [--cached]`

Remove from the index and (without `--cached`) from the working tree. `path_not_tracked` for unknown paths.

```json
{"ok":true,"result":{"removed":["old.txt"]}}
```

### `agit reset [ARGS]... [--soft|--mixed|--hard]`

Two forms. `agit reset <paths>` unstages (`{"unstaged":[…]}`). `agit reset [--soft|--mixed|--hard] <rev>` moves the current branch: `--soft` only the branch, `--mixed` (default when a revision is given) also the index, `--hard` also the working tree. Logged in the reflog; undo with the previous id.

```json
{"ok":true,"result":{"from":"fa32a426…","to":"a109bac6…","mode":"hard","files_changed":2}}
```

### `agit status [-s|--short]`

```json
{"ok":true,"result":{"workspace":"main","branch":"main","head":"edf2c5ac…","staged":[{"path":"src/config.rs","kind":"added","new":{"id":"49537…","mode":"file","size":126}}],"unstaged":[{"path":"src/main.rs","kind":"modified","old":{…},"new":{…}}],"untracked":["notes.txt"],"conflicts":[],"merge_in_progress":false}}
```

`branch` is `null` when detached. Change kinds: `added`, `modified`, `deleted`, `typechange`, `renamed`. `--short` prints git-like two-column lines (`M `, ` M`, `A `, `UU`, `??`).

### `agit commit -m <MESSAGE> [--intent S] [--task S] [--meta K=V]... [-a|--all] [--allow-empty] [--force] [--sign [KEY]]`

Record the index as a commit. `-a` stages modifications of tracked files first. `--force` ignores other agents' exclusive leases. `--sign` with no value uses the default key name. Fails with `nothing_to_commit`, `lease_conflict` (exit 3) or, during a merge with unresolved paths, `invalid`.

```json
{"ok":true,"result":{"id":"edf2c5ac…","tree":"560556cb…","parents":["44b9a0f2…"],"branch":"feature/config","changes":[{"path":"src/config.rs","kind":"added","new":{…}}],"summary":"Load the greeting from Config"}}
```

### `agit checkpoint [-m MESSAGE] [--if-changed]`

Snapshot the whole working tree onto `refs/checkpoints/<workspace>`.

```json
{"ok":true,"result":{"id":"4cc2e92e…","tree":"b37c8a4e…","previous":null,"changes_vs_head":2,"created":true}}
```

`agit checkpoint list [-n 20]` returns `[{id, tree, parents, author, time_ms, message, checkpoint: true, meta?}]` newest first. `agit checkpoint restore <REV>` and `agit restore <REV>` return the list of applied changes (same shape as `status.staged` entries) and overwrite local modifications.

### `agit restore <REV>`

Working tree to any commit or checkpoint. See above.

### `agit blame <PATH> [REV] [--max-commits 500]`

```json
{"ok":true,"result":{"path":"src/main.rs","rev":"fa32a426…","lines":[{"line":1,"commit":"f61ab45c…","text":"fn main() {"}],"commits":{"f61ab45c…":{"id":"f61ab45c…","author":"Dana Developer","agent":null,"time_ms":1792092451181,"summary":"Initial commit","intent":"Bootstrap the demo project"}},"truncated":false}}
```

### `agit revert <REV>` and `agit cherry-pick <REV>`

Apply the inverse (or the change) of `REV` on top of HEAD as a new commit, three-way. Result is either `{"kind":"committed","commit":…,"files_changed":n}` or `{"kind":"conflicts","conflicts":[…]}` with exit 3.

### `agit stash [push [-m MESSAGE] | list | pop [INDEX] | apply [INDEX] | drop [INDEX]]`

No subcommand means `push`. Stashes include untracked files and reset the tree to HEAD.

```json
{"ok":true,"result":{"index":0,"id":"85b65fe0…","message":"WIP on main","time_ms":1792092601040,"base":"a109bac6…"}}
{"ok":true,"result":[{"index":0,"id":"…","message":"WIP on main","time_ms":…,"base":"…"}]}
{"ok":true,"result":{"kind":"committed","commit":"85b65fe0…","files_changed":0}}
{"ok":true,"result":{"dropped":"85b65fe0…"}}
```

### `agit grep <PATTERN> [--rev REV] [-i|--ignore-case] [--max-file-bytes 1048576]`

Regex over the working tree or a revision; exit 1 when nothing matches.

```json
{"ok":true,"result":[{"path":"src/main.rs","line":2,"text":"    println!(\"hello, world and agit\");"}]}
```

### `agit log [REV] [-n N] [--agent ID] [--task ID] [--path P] [--first-parent] [--oneline] [--graph]`

```json
{"ok":true,"result":[{"id":"83e741ad…","tree":"c7c4bd34…","parents":["b906dfda…"],"author":{"name":"Dana Developer","email":"dana@example.com"},"time_ms":1792092645310,"message":"Punctuation","intent":"Add exclamation mark","agent":{"id":"coder-1","model":"demo-model","run_id":"run-001"}}]}
```

Entries are the commit object plus `id`; optional fields are omitted. On an unborn branch the result is `[]`. `--graph` draws an ASCII graph in human mode.

### `agit show [REV] [--no-patch]`

For a commit: `{"id", "commit": {…}, "changes": [...], "patch": "unified diff or null"}`. For a tree: the entry list. For a blob: raw bytes in human mode, `{"id","size","binary","content"}` with `--json`.

### `agit diff [REVS]... [--staged] [--stat] [--name-only] [-U|--unified 3]`

No revision: working tree vs index. `--staged` (alias `--cached`): index vs HEAD. One revision: that commit vs working tree. Two: commit vs commit. At most two.

```json
{"ok":true,"result":{"changes":[{"path":"src/main.rs","kind":"modified","old":{"id":"…","mode":"file","size":43},"new":{"id":"…","mode":"file","size":53}}],"stats":[{"path":"src/main.rs","added":1,"removed":1}],"patch":"--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,3 @@\n…"}}
```

Renames are detected at 50% similarity (`kind: "renamed"`, `from`, `similarity`). `patch` is `null` with `--stat` or `--name-only`.

### `agit branch [NAME] [START] [-d|--delete] [-D]`

List (`[{name, id, current}]`), create (`{name, id, current:false}`) or delete (`{deleted, was}`). `-d` refuses unmerged branches, `-D` forces.

### `agit switch <TARGET> [-c|--create] [-f|--force]` (alias `checkout`)

Switch branches or detach at a revision. Refuses to overwrite local modifications (`dirty_worktree`, exit 3) unless `--force`.

```json
{"ok":true,"result":{"head":{"kind":"branch","name":"feature/config"},"id":"44b9a0f2…","files_changed":0}}
{"ok":true,"result":{"head":{"kind":"detached"},"id":"837fc349…","files_changed":1}}
```

### `agit merge [REV] [-m MESSAGE] [--intent S] [--no-ff] [--abort] [--force] [--sign [KEY]]`

Results: `{"kind":"already_up_to_date","head":…}`, `{"kind":"fast_forward","to":…,"files_changed":n}`, `{"kind":"merged","commit":…,"tree":…,"auto_merged":[…]}`, or `{"kind":"conflicts","tree":…,"conflicts":[{"path","kind":"content|delete_modify|add_add|mode|binary","base?","ours?","theirs?"}],"auto_merged":[…]}` followed by the `merge_conflicts` error envelope (exit 3). `--abort` returns `{"aborted":true}`. `--force` ignores leases on the incoming paths.

### `agit tag [NAME] [REV] [-d|--delete]`

List `[{name, id}]`, create `{name, id}`, delete `{deleted, was}`. Tags are local only in 0.1.0 (not pushed).

### `agit rev-parse <REV>`

```json
{"ok":true,"result":{"rev":"HEAD","id":"f61ab45c…"}}
```

### `agit cat-file <ID> [-t]`

`-t` prints the type (`{"id","type":"blob|tree|commit"}`). Otherwise raw bytes for blobs, entries for trees, the commit JSON for commits (including `signature` when present). Accepts ids, prefixes and revisions.

### `agit ls-tree [REV] [-r|--recursive]`

Entries `[{name|path, mode, id, size}]`; with `-r` paths are full and directories are omitted.

### `agit fsck`

```json
{"ok":true,"result":{"objects":34,"corrupt":[],"missing":[],"dangling_refs":[]}}
```

Exit 1 when any list is non-empty. Submodule pins are not treated as missing.

### `agit gc [--dry-run] [--grace 120] [--reflog-expire 90d] [--checkpoint-expire 30d]`

```json
{"ok":true,"result":{"objects_total":37,"objects_reachable":31,"objects_removed":3,"bytes_removed":421,"objects_kept_young":3,"reflog_entries_expired":0,"checkpoints_expired":0,"dry_run":false}}
```

Reachability includes branches, tags, checkpoints, stashes, the index and unexpired reflog entries. Loose objects are repacked into segments.

### `agit reflog [NAME] [-n N]`

`NAME` is `HEAD` (default), a branch, or a full `refs/...` name.

```json
{"ok":true,"result":[{"old":"fa32a426…","new":"a109bac6…","actor":"danielbrueckmann","time_ms":1792092601121,"message":"reset: moving to HEAD~1"}]}
```

### `agit events [--since 0] [-f|--follow] [--kind PREFIX]`

One event per line in JSON mode (no envelope):

```json
{"seq":29,"time_ms":1792092645330,"kind":"lease.release","workspace":"main","actor":"coder-1","data":{"id":"c618a7ee7a6c","paths":["src/main.rs"]}}
```

Human mode: `#<seq> <time> <kind> <workspace> <actor> <data>`.

## Coordination commands

### `agit lease claim [PATHS]... --holder H [--ttl 1h] [--note S] [--shared]`

```json
{"ok":true,"result":{"id":"3bd2ec0d8a15","holder":"coder-1","paths":["src"],"mode":"exclusive","created_ms":1792092751063,"expires_ms":1792094551063,"note":"add config loading (DEMO-7)","workspace":"main"}}
```

Literal paths are made repository-relative from the current directory; patterns containing `* ? [ {` are kept verbatim. `--holder` defaults to `AGIT_AGENT` and is required. With an HTTP remote the lease is claimed on the server first (team-wide; a conflict with an agent on another machine is `lease_conflict`) and mirrored locally with `"remote":"origin"`; the human output ends with `(shared via origin)` or `(this machine only)`.

### `agit lease release [ID] [--holder H] [--all]`

Returns the released leases as a list. `--all` needs `--holder`.

### `agit lease renew <ID> [--ttl 1h]`

Returns the lease with the new `expires_ms`. `lease_not_found` when expired or unknown.

### `agit lease list`

`[lease]`, active only, sorted by `created_ms`: the server's leases (the whole team, `remote` set) merged with this repository's local-only ones. The human output marks each `shared` or `local`.

### `agit lease check [PATHS]... --holder H`

`{"allowed":true,"holder":"coder-2","paths":["src/main.rs"]}` or the `lease_conflict` error (exit 3). Claims nothing. Checks the local store and the server.

### `agit lease sync`

Registers leases claimed while the server was unreachable, reports the ones the server refused because someone else holds the paths (they stay local), and drops local mirrors of leases that were released or expired elsewhere: `{"registered":[lease],"refused":[[lease,"holder"]],"dropped":["id"],"server_leases":n}`.

### `agit ws new <NAME> [--path DIR] [--from REV] [--agent ID]` (alias `workspace`)

```json
{"ok":true,"result":{"name":"coder-2","path":"/…/demo.coder-2","created_ms":1792092645238,"agent":"coder-2"}}
```

`agit ws list` returns `[{name, path, created_ms, agent?}]` (the primary workspace is marked `*` in human mode). `agit ws remove <NAME> [--delete]` returns the removed metadata.

## Remote commands

### `agit remote add <NAME> <URL>` / `list` / `remove <NAME>`

URL form: `http(s)://host:port/api/v1/repos/<repo>`. `list` returns `[{name, url}]`.

### `agit clone <URL> [DEST]`

```json
{"ok":true,"result":{"path":"/…/demo-b","fetch":{"remote":"origin","refs":[{"name":"refs/remotes/origin/main","old":null,"new":"44b9a0f2…"}],"objects_received":33}}}
```

Creates `DEST` (default: repository name), sets `origin`, checks out the default branch. Refuses a non-empty destination.

### `agit push [REMOTE] [BRANCH] [-f|--force] [--create]`

```json
{"ok":true,"result":{"remote":"origin","branch":"main","old":null,"new":"44b9a0f2…","objects_sent":33,"up_to_date":false}}
```

`--create` creates the remote repository when missing. A non-fast-forward push fails client-side with `invalid` ("run `agit pull` first or push with --force"); a server-side race is `ref_conflict`; a protected branch yields `policy_violation`. Tags are not pushed.

### `agit fetch [REMOTE] [BRANCHES]...`

```json
{"ok":true,"result":{"remote":"origin","refs":[{"name":"refs/remotes/origin/main","old":"44b9…","new":"3bf5…"}],"objects_received":10}}
```

### `agit pull [REMOTE] [--no-ff]`

Fetch the current branch and merge it: `{"fetch": {...}, "merge": <merge result>}`; conflicts exit 3. Refuses with `dirty_worktree` when the merge would overwrite local changes.

### `agit auth [--url BASE] [--remote origin] <login|logout|whoami|show-token|key|token>`

The server is `--url` (an `…/api/v1` base or any repository URL on it), else the remote of the repository in the current directory, else `AGIT_SERVER_URL`.

- `login [--key PATH] [--ttl 8h] [--print-token]`: signs a server challenge with an SSH key (`--key`, `AGIT_SSH_KEY`, `~/.ssh/id_ed25519`, ssh-agent, or an agit key) and caches the session. Result: `{server, principal, agent, scopes, key, fingerprint, source, expires_ms, token?}`. An unknown key fails with `unauthorized` and the message `… is not authorized on <server>`.
- `logout`: revokes the cached session (`{revoked}`). `whoami`: `{principal, agent, root, scopes, key}`. `show-token`: `{server, token, expires_ms, agent}` for the web UI or an SDK.
- `key add <PUBKEY|FILE> [--name S] [--agent ID] [--scope repo:access]... [--admin]`, `key list`, `key remove <ID>` (admin). `--scope` takes `repo:read|write|admin` or `*:…`; `--admin` is `*:admin`. A file adds every key in it. Results are the authorized-key objects of `GET /auth/keys`; `remove` returns `{deleted, revoked_sessions}`.
- `token create --name S [--agent ID] [--scope …]... [--admin] [--ttl 30d]`, `token list`, `token revoke <ID>` (admin): the token objects of `/auth/tokens`; `create` includes the plaintext `token` once.

## Platform commands (pull requests)

All `pr` subcommands take `--remote <name>` (default `origin`) and act as `AGIT_AGENT`.

### `agit pr create -t <TITLE> [-b BODY] [--intent S] [--task S] [--source BRANCH] [--target BRANCH] [--push]`

Returns the PR object ([8](08-http-api.md#pull-request-lifecycle)). `--source` defaults to the current branch, `--target` to the default branch; `--push` pushes the source first.

### `agit pr list [--state open|merged|closed]`

`[pr]`, newest first.

### `agit pr view <NUMBER>`

`{"pr": {...}, "commits": [{id, commit}], "changes": [...], "mergeable": bool, "conflicts": [...]}`.

### `agit pr review <NUMBER> [--decision approve|request_changes|comment] [-b BODY]`

Returns the PR with the new review appended.

### `agit pr comment <NUMBER> -b <BODY> [--path P] [--line N] [--kind question|suggestion|blocker|note]`

Returns the comment `{id, author, path, line, kind, body, resolved:false, time_ms}`. Resolving a comment is HTTP only (`POST /pulls/{n}/comments/{id}/resolve`).

### `agit pr merge <NUMBER> [--strategy merge|ff] [-m MESSAGE]`

`{"pr": {...state: "merged"...}, "merged_commit": "…"}`; `policy_violation`, `merge_conflicts` or `ref_conflict` on failure.

### `agit pr close <NUMBER>`

Returns the PR with `state: "closed"`.

## Knowledge commands

All `know` subcommands take `--remote <name>` and `--local`. Without `--local` the server is used when the remote is configured and reachable; on a transport error (or no remote) the local store `.agit/knowledge/` answers and `source` is `"local"`.

### `agit know add -t <TITLE> [-b BODY | --body-file F|-] [--tag NS:V]... [--path GLOB]... [--commit REV]... [--confidence F]`

```json
{"ok":true,"result":{"note":{"id":"n_1f5d3dce7c3b","title":"…","body":"…","tags":["kind:gotcha","area:core"],"paths":["src/config.rs"],"commits":["edf2c5ac…"],"pulls":[],"author":"coder-1","confidence":0.8,"created_ms":…,"updated_ms":…,"valid_at":"3bf5d89c…","stale":false},"source":"server"}}
```

### `agit know search [--tag NS:V]... [--path P] [-q|--query TEXT] [-n 20]`

```json
{"ok":true,"result":{"notes":[{"id":"n_…","title":"…","tags":[…],"paths":[…],"author":"coder-1","confidence":0.8,"updated_ms":…,"excerpt":"first 200 chars","valid_at":"…","draft":false,"stale":false,"score":2.09}],"source":"server"}}
```

`score` is present only with `-q`.

### `agit know get <ID>`

The full note (no `source` wrapper).

### `agit know context <PATH>`

```json
{"ok":true,"result":{"context":{"path":"src/config.rs","notes":[…summaries…],"suggested_tags":["area:core","lang:rust"],"recent_commits":[{id, …commit}],"leases":[…],"open_pulls":[…],"agents_active":["coder-1","reviewer-1"]},"source":"server"}}
```

### `agit know tags`

`[{tag, count}]` over accepted notes (no wrapper).

### `agit know sync`

```json
{"ok":true,"result":{"pushed":1,"pulled":9,"rules_pulled":2}}
```

Uploads `n_local_*` notes, refreshes the cache of server notes and tag rules. Needs a remote.

## Key commands

Keys are stored in `~/.agit/keys` or `$AGIT_KEYS_DIR`.

### `agit key generate [NAME] [--register] [--remote origin] [--agent ID]`

```json
{"ok":true,"result":{"key":{"name":"coder-1","key_id":"0ae983c0881ef763","public_key":"da2e81ef…"},"path":"/…/keys/coder-1.key","registered":{"agent":"coder-1","key_id":"0ae983c0881ef763","public_key":"da2e81ef…","added_ms":…,"added_by":"coder-1"}}}
```

`registered` is `null` without `--register`. Default name: `AGIT_KEY`, else `AGIT_AGENT`, else `default`.

### `agit key list` / `agit key show [NAME]` / `agit key delete <NAME>`

`[{name, key_id, public_key}]` / `{name, key_id, public_key}` / `{deleted}`.

### `agit key register [NAME] [--remote origin] [--agent ID]`

Registers the public half with the remote repository (admin scope). Returns the key entry.

### `agit verify [REV]`

```json
{"ok":true,"result":{"id":"486a50ed…","signature":{"status":"valid","key_id":"0ae983c0881ef763","signer":"coder-1","public_key":"da2e81ef…"}}}
```

`status` is `unsigned`, `valid` or `invalid {reason}`; unsigned and invalid exit 1 (an `error` envelope follows the result in JSON mode). Trust (registry membership) is a server concept; see `GET /commits/{rev}`.

## Bridge commands

### `agit git import <SOURCE> [--into DIR] [--branch NAME]... [--no-tags] [--no-checkout]`

```json
{"ok":true,"result":{"commits":2,"trees":2,"blobs":1,"branches":["main"],"tags":[],"skipped_commits":0}}
```

### `agit git export <TARGET> [--branch NAME]... [--no-tags]`

```json
{"ok":true,"result":{"commits":2,"reused":0,"branches":["main"],"tags":[]}}
```

Both shell out to `git`; relative paths resolve against `-C`.

### `agit submodule add <URL> <PATH> [--branch B]` / `update [PATHS]...` / `status` / `remove <PATH> [--delete]`

```json
{"ok":true,"result":{"path":"vendor/lib","url":"../lib"}}
{"ok":true,"result":[{"path":"vendor/lib","action":"cloned","commit":"52a06ffb…"},{"path":"vendor/lib","action":"checked_out","commit":"2f43cacd…"}]}
{"ok":true,"result":[{"path":"vendor/lib","url":"../lib","pinned":"2f43cacd…","actual":"2f43cacd…","state":"ok"}]}
{"ok":true,"result":{"removed":"vendor/lib"}}
```

## Agent integration

### `agit mcp [--remote origin]`

Runs the MCP server over stdio (JSON-RPC 2.0, one message per line) in the current or `-C` directory. See [7](07-mcp-reference.md).

## Maintenance summary

| Task | Command |
| --- | --- |
| verify integrity | `agit fsck` |
| reclaim space, expire checkpoints and reflog | `agit gc [--dry-run]` |
| inspect an object | `agit cat-file <id>`, `agit show <id>` |
| find where HEAD was | `agit reflog` |
| see who is working where | `agit lease list`, `agit ws list`, `agit events --kind lease.` |
| rebuild a broken index | `agit restore HEAD` (rewrites index and tree from HEAD; discards local changes) |
