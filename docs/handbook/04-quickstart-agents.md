# 4. Quick start for agents

This chapter runs the agent loop end to end against a real `agit-server`: identity, context, lease, checkpoint, commit with intent and task, pull request, review by a second agent, merge under a policy, a knowledge note, and validation of a note that went stale. Every step has a CLI form (shown here with `--json`), an HTTP form ([8](08-http-api.md)), an MCP tool ([7](07-mcp-reference.md)) and an SDK method ([9](09-sdks.md)). The same loop is written as a one-page briefing in [docs/AGENT-GUIDE.md](../AGENT-GUIDE.md).

The loop in one sentence: **read context, claim what you will change, checkpoint before risk, commit with intent, open a PR, record what you learned, release.**

## 0. Start a server and push a repository

```sh
$ export AGIT_SERVER_TOKEN=handbook-demo-token
$ agit-server --data-dir ./server-data --listen 127.0.0.1:17979 &
$ curl -s -H "Authorization: Bearer $AGIT_SERVER_TOKEN" http://127.0.0.1:17979/api/v1/health
{"status":"ok","version":"0.1.0"}
```

In an existing local repository (the one from [chapter 3](03-quickstart-developers.md)):

```sh
$ export AGIT_TOKEN=handbook-demo-token
$ agit remote add origin http://127.0.0.1:17979/api/v1/repos/demo --json
{"ok":true,"result":{"name":"origin","url":"http://127.0.0.1:17979/api/v1/repos/demo"}}
$ agit push --create --json
{"ok":true,"result":{"remote":"origin","branch":"main","old":null,"new":"44b9a0f2…","objects_sent":33,"up_to_date":false}}
```

Agents usually start from a clone instead:

```sh
$ agit clone http://127.0.0.1:17979/api/v1/repos/demo demo-b --json
{"ok":true,"result":{"path":"/…/demo-b","fetch":{"remote":"origin","refs":[{"name":"refs/remotes/origin/main","old":null,"new":"44b9a0f2…"}],"objects_received":33}}}
$ cd demo-b
```

## 1. Identify yourself

Set the identity once per run. Every commit, lease, review and note carries it; `agit log --agent`, the server's `/agents` endpoint and the web UI's Agents tab find your work later.

```sh
$ export AGIT_AGENT=coder-1 AGIT_MODEL=demo-model-1 AGIT_RUN_ID=run-2026-10-15-a
$ agit config user.name coder-1 && agit config user.email coder-1@agents.example
```

`AGIT_AGENT` also becomes the lease holder, the `X-Agit-Agent` header on every request, and the default name of your signing key. Prefer a stable id per agent role (`coder-1`, `reviewer-1`), and put the per-run correlation in `AGIT_RUN_ID`. If the operator gave you a scoped token bound to your agent id ([13](13-server-administration.md#scoped-tokens)), the server records that identity regardless of the header.

## 2. Read before you touch

One call returns the notes covering a path (fresh first, then stale), the tags a note about this path should carry, recent commits, leases, open pull requests and the agents active there:

```sh
$ agit know context src/main.rs --json
{"ok":true,"result":{"context":{"path":"src/main.rs","notes":[],"suggested_tags":[],"recent_commits":[{"id":"83e741ad…","message":"Punctuation","intent":"Add exclamation mark","agent":{"id":"coder-1","model":"demo-model","run_id":"run-001"},…},…],"leases":[],"open_pulls":[],"agents_active":["Dana Developer","coder-1"]},"source":"server"}}
```

Nothing is tagged yet because the repository has no tag rules. An operator (or you, with an admin token) sets them once; after that the context call suggests tags:

```sh
$ curl -s -X PUT -H "Authorization: Bearer $AGIT_TOKEN" -H 'content-type: application/json' \
    http://127.0.0.1:17979/api/v1/repos/demo/knowledge/rules \
    -d '[{"pattern":"src/**","tags":["area:core","lang:rust"]},{"pattern":"vendor/**","tags":["area:vendor"]}]'
$ agit know context src/main.rs
context for src/main.rs
suggested tags: area:core, lang:rust

notes (0):

recent commits (5):
  83e741ad27e1 Punctuation (coder-1)
  …
leases (0):

open pull requests (0):
```

`source` is `server` when the answer came from the remote and `local` when it came from the offline cache in `.agit/knowledge/` (see [11](11-knowledge-base.md#offline-local-store-and-sync)).

## 3. Claim what you will change

```sh
$ agit lease claim src --ttl 30m --note "add config loading (DEMO-7)" --json
{"ok":true,"result":{"id":"3bd2ec0d8a15","holder":"coder-1","paths":["src"],"mode":"exclusive","created_ms":1792092751063,"expires_ms":1792094551063,"note":"add config loading (DEMO-7)","workspace":"main"}}
```

A literal path covers itself and everything beneath it; globs (`src/**/*.rs`) work too. Another agent's claim on an overlapping path now fails with `lease_conflict` (exit 3), and so does their `agit commit` touching `src/…`. Leases expire by themselves, so a crashed agent never blocks anyone for long; renew with `agit lease renew <id> --ttl 30m` while you keep working, and release early with `agit lease release --all` when you are done. `--shared` creates an informational lease that blocks nobody (useful for "I am reading this").

With a remote, the claim lands on the server at once, so agents on other machines see it (`agit lease list`, the web UI's lease board, the `context` endpoint) and their commits into `src/` are refused too; a copy stays in `.agit/leases/` for the workspaces on this machine and for offline work. Without a server the lease lives only in `.agit/leases/`.

## 4. Work on a branch or in your own workspace

```sh
$ agit switch -c feature/config --json
{"ok":true,"result":{"head":{"kind":"branch","name":"feature/config"},"id":"44b9a0f2…","files_changed":0}}
```

When several agents share one checkout, give each its own workspace instead (`agit ws new coder-1`), which is a separate directory on branch `ws/coder-1` sharing the object store ([5](05-concepts.md#workspaces)).

## 5. Checkpoint before risky edits

```sh
$ agit checkpoint -m "before adding config" --json
{"ok":true,"result":{"id":"2156bbe5…","tree":"9b383d27…","previous":null,"changes_vs_head":1,"created":true}}
```

Restore with `agit restore <id>` (whole working tree, untracked files included). Checkpoints are cheap and invisible in branch history; a cadence of "one before every destructive step and one at the end of every turn" is what the Claude Code integration enforces automatically.

## 6. Commit with intent

```sh
$ printf 'pub struct Config { pub greeting: String }\n\nimpl Config {\n    pub fn load() -> Self { Config { greeting: "hello".into() } }\n}\n' > src/config.rs
$ printf 'mod config;\n\nfn main() {\n    let cfg = config::Config::load();\n    println!("{}, world and agit!", cfg.greeting);\n}\n' > src/main.rs
$ agit add -A --json
{"ok":true,"result":{"added":["src/config.rs"],"updated":["src/main.rs"],"removed":[],"unchanged":3}}
$ agit commit -m "Load the greeting from Config" --intent "Make the greeting configurable for DEMO-7" --task DEMO-7 --meta ci.tests=pending --json
{"ok":true,"result":{"id":"edf2c5ac…","tree":"560556cb…","parents":["44b9a0f2…"],"branch":"feature/config","changes":[{"path":"src/config.rs","kind":"added",…},{"path":"src/main.rs","kind":"modified",…}],"summary":"Load the greeting from Config"}}
```

`message` says what, `intent` says why, `task` links the tracker, `--meta key=value` adds anything else (the value is parsed as JSON when it can be). The agent identity came from the environment. If a signing key named after your agent exists, the commit is signed automatically ([step 10](#10-sign-your-commits)).

## 7. Push and open a pull request

```sh
$ agit pr create --push -t "Load greeting from Config" -b "First step towards configurable output." \
    --intent "Make the greeting configurable for DEMO-7" --task DEMO-7 --json
{"ok":true,"result":{"number":1,"title":"Load greeting from Config","body":"First step towards configurable output.","intent":"Make the greeting configurable for DEMO-7","task":"DEMO-7","source":"refs/heads/feature/config","target":"refs/heads/main","source_id":"edf2c5ac…","target_id":"44b9a0f2…","base_id":"44b9a0f2…","author":{"id":"coder-1"},"state":"open","created_ms":1792092751113,"updated_ms":1792092751113,"reviews":[],"comments":[],"checks":[]}}
$ agit pr view 1 --json | jq '.result | {mergeable, conflicts, commits: (.commits|length), changes: [.changes[].path]}'
{"mergeable": true, "conflicts": [], "commits": 1, "changes": ["src/config.rs", "src/main.rs"]}
```

`--push` pushes the source branch first (compare-and-swap on the remote ref). Without `--source` the current branch is used; without `--target` the repository's default branch.

## 8. Review by a second agent

In another clone, as `reviewer-1`. Comments have a kind (`question`, `suggestion`, `blocker`, `note`) and can be anchored to a path and line; reviews are `approve`, `request_changes` or `comment`:

```sh
$ export AGIT_AGENT=reviewer-1
$ agit pr comment 1 --kind blocker --path src/config.rs --line 4 -b "Hard-coded greeting; read it from the environment." --json
{"ok":true,"result":{"id":1,"author":"reviewer-1","path":"src/config.rs","line":4,"kind":"blocker","body":"Hard-coded greeting; read it from the environment.","resolved":false,"time_ms":1792092753218}}
$ agit pr review 1 --decision request_changes -b "See blocker on config.rs" --json | jq -c '.result.reviews'
[{"reviewer":"reviewer-1","decision":"request_changes","body":"See blocker on config.rs","time_ms":1792092753229}]
```

## 9. Merge under a policy

An operator protects `main`: changes only through pull requests, one approval, intent and agent identity required on every commit. (`PUT /policy` is an admin operation; see [13](13-server-administration.md#policies).)

```sh
$ curl -s -X PUT -H "Authorization: Bearer $AGIT_TOKEN" -H 'content-type: application/json' \
    http://127.0.0.1:17979/api/v1/repos/demo/policy \
    -d '{"protected_branches":["main"],"require_pull_request":true,"require_approvals":1,"require_intent":true,"require_agent":true}'
{"protected_branches":["main"],"require_pull_request":true,"require_approvals":1,"require_intent":true,"require_agent":true,"require_checks":false,"block_on_blockers":true,"allowed_merge_agents":[],"allowed_push_agents":[],"max_files_per_commit":0,"require_signed":false}
```

A direct push to `main` is now refused, and so is the merge while changes are requested:

```sh
$ curl -s -X POST … /repos/demo/refs/update -d '{"updates":[{"name":"refs/heads/main","expect":{"any":true},"new":"edf2c5ac…"}]}'
{"ok":false,"error":{"code":"policy_violation","message":"policy violation: refs/heads/main is protected; changes must go through a pull request","retryable":false}}
$ agit pr merge 1 --json; echo "exit=$?"
{"ok":false,"error":{"code":"policy_violation","message":"policy violation: policy violation: a reviewer requested changes","retryable":false,"details":null}}
exit=1
```

The reviewer resolves the blocker, a CI agent reports a check, the reviewer approves, and the merge goes through. Merging happens on the server (three-way merge, or `--strategy ff`) with a compare-and-swap on the target branch:

```sh
$ curl -s -X POST -H "Authorization: Bearer $AGIT_TOKEN" -H 'X-Agit-Agent: reviewer-1' http://127.0.0.1:17979/api/v1/repos/demo/pulls/1/comments/1/resolve >/dev/null
$ curl -s -X POST -H "Authorization: Bearer $AGIT_TOKEN" -H 'X-Agit-Agent: ci-bot' -H 'content-type: application/json' \
    http://127.0.0.1:17979/api/v1/repos/demo/pulls/1/checks -d '{"name":"tests","status":"success","url":"https://ci.example/run/42"}' >/dev/null
$ agit pr review 1 --decision approve -b "Resolved; fine as a first step." --json | jq -c '.result.reviews | map(.decision)'
["request_changes","approve"]
$ agit pr merge 1 --json
{"ok":true,"result":{"pr":{"number":1,…,"state":"merged","merged_commit":"3bf5d89c…","reviews":[…],"comments":[{"id":1,…,"resolved":true,…}],"checks":[{"name":"tests","status":"success","url":"https://ci.example/run/42","time_ms":1792092786991}]},"merged_commit":"3bf5d89c…"}}
```

Only the latest decision per reviewer counts, the PR author's approval does not, any `request_changes` blocks, and unresolved `blocker` comments block while `block_on_blockers` is on. A `ref_conflict` from the merge means someone else moved `main` first; re-check `agit pr view` and retry.

Back in the author's clone, `agit switch main && agit pull` fast-forwards to the merge (in this walkthrough the repository contains a submodule entry, which trips the switch bug noted in the [FAQ](15-faq.md#known-discrepancies-and-rough-edges); a fresh clone shows the merged state).

## 10. Sign your commits

Signing is optional unless a policy demands it. Generate a key named after your agent and register its public half with the repository in one step; from then on every commit you make is signed automatically:

```sh
$ agit key generate --register --json
{"ok":true,"result":{"key":{"name":"coder-1","key_id":"0ae983c0881ef763","public_key":"da2e81ef…"},"path":"/…/keys/coder-1.key","registered":{"agent":"coder-1","key_id":"0ae983c0881ef763","public_key":"da2e81ef…","added_ms":1792092892004,"added_by":"coder-1"}}}
$ agit commit -m "Read GREETING from the environment" --intent "Resolve the blocker from PR #1 (DEMO-8)" --task DEMO-8 --json | jq -c '.result | {id, summary}'
{"id":"486a50ed…","summary":"Read GREETING from the environment"}
$ agit verify
486a50ed42be: valid signature by coder-1 (key 0ae983c0881ef763)
$ curl -s -H "Authorization: Bearer $AGIT_TOKEN" http://127.0.0.1:17979/api/v1/repos/demo/commits/feature%2Fenv | jq -c .signature
{"status":"valid","key_id":"0ae983c0881ef763","signer":"coder-1","public_key":"da2e81ef…","trusted":true}
```

Keys live in `~/.agit/keys/<name>.key` (or `$AGIT_KEYS_DIR`); `AGIT_SIGN=off` disables automatic signing, `--sign [name]` forces it. With `require_signed` in the policy, an unsigned commit cannot reach `main` through a PR:

```sh
$ agit pr merge 3 --json
{"ok":false,"error":{"code":"policy_violation","message":"policy violation: policy violation: commit f7d8f7966e49 is not signed (require_signed)","retryable":false,"details":null}}
```

## 11. Record what you learned

```sh
$ agit know add -t "Config::load() is a stub" --tag kind:gotcha --tag area:core --tag lang:rust \
    --path src/config.rs --commit HEAD --confidence 0.8 \
    -b "The greeting is hard-coded until DEMO-8 wires it to the environment. Do not rely on it for localisation." --json
{"ok":true,"result":{"note":{"id":"n_1f5d3dce7c3b","title":"Config::load() is a stub","body":"…","tags":["kind:gotcha","area:core","lang:rust"],"paths":["src/config.rs"],"commits":["edf2c5ac…"],"pulls":[],"author":"coder-1","confidence":0.8,"created_ms":…,"updated_ms":…,"valid_at":"3bf5d89c…","stale":false},"source":"server"}}
$ agit know tags --json
{"ok":true,"result":[{"tag":"area:core","count":2},{"tag":"kind:decision","count":1},{"tag":"kind:gotcha","count":1},{"tag":"lang:rust","count":1}]}
$ agit know search -q "hard-coded greeting" --json | jq -c '.result.notes[] | {title, score}'
{"title":"Config::load() is a stub","score":2.0951898097991943}
{"title":"Greeting text lives in Config, not main.rs","score":0.2148577719926834}
```

Use the `suggested_tags` from the context call plus a `kind:` tag (`gotcha`, `decision`, `spec`, `howto`, `fact`, `rule`). `valid_at` is set to the server's HEAD at creation and is what staleness is measured against.

Every merged pull request also leaves a **draft note** (hidden until accepted) summarising the change; accept it, edit it or delete it:

```sh
$ curl -s -H "Authorization: Bearer $AGIT_TOKEN" "http://127.0.0.1:17979/api/v1/repos/demo/knowledge/notes?drafts=1&pull=1" | jq -c '.[] | {id, title, draft, tags, paths, confidence}'
{"id":"n_599ca51c9760","title":"Load greeting from Config","draft":true,"tags":["kind:change","area:core","lang:rust"],"paths":["src/**"],"confidence":0.4}
$ curl -s -X POST -H "Authorization: Bearer $AGIT_TOKEN" -H 'X-Agit-Agent: coder-1' -H 'content-type: application/json' \
    http://127.0.0.1:17979/api/v1/repos/demo/knowledge/notes/n_599ca51c9760/accept -d '{"title":"PR #1: greeting moved into Config","confidence":0.7}' | jq -c '{id, title, draft}'
{"id":"n_599ca51c9760","title":"PR #1: greeting moved into Config","draft":null}
```

## 12. Validate stale notes

After PR #4 changed `src/config.rs`, every note covering that path is stale. `stale` is computed on every read by diffing the note's `valid_at` against HEAD; the stale endpoint lists the changed paths:

```sh
$ curl -s -H "Authorization: Bearer $AGIT_TOKEN" http://127.0.0.1:17979/api/v1/repos/demo/knowledge/stale | jq -c '.[] | {id, title, changed_paths}'
{"id":"n_599ca51c9760","title":"PR #1: greeting moved into Config","changed_paths":["src/config.rs"]}
{"id":"n_5a5d76aedccb","title":"Greeting text lives in Config, not main.rs","changed_paths":["src/config.rs"]}
{"id":"n_1f5d3dce7c3b","title":"Config::load() is a stub","changed_paths":["src/config.rs"]}
$ agit know context src/config.rs --json | jq -c '.result.context.notes | map({title, stale})'
[{"title":"PR #1: greeting moved into Config","stale":true},…]
```

The agent who made the change reads each note, fixes the text where it is wrong, and confirms it at HEAD:

```sh
$ curl -s -X PATCH -H "Authorization: Bearer $AGIT_TOKEN" -H 'X-Agit-Agent: coder-1' -H 'content-type: application/json' \
    http://127.0.0.1:17979/api/v1/repos/demo/knowledge/notes/n_1f5d3dce7c3b \
    -d '{"title":"Config::load() reads GREETING from the environment","body":"Since DEMO-8 the greeting comes from $GREETING with a fallback to \"hello\". Localisation still needs a locale-aware source."}' | jq -c '{title, stale, valid_at: .valid_at[0:12]}'
{"title":"Config::load() reads GREETING from the environment","stale":true,"valid_at":"3bf5d89c19f4"}
$ curl -s -X POST -H "Authorization: Bearer $AGIT_TOKEN" -H 'X-Agit-Agent: coder-1' -H 'content-type: application/json' \
    http://127.0.0.1:17979/api/v1/repos/demo/knowledge/notes/n_1f5d3dce7c3b/validate -d '{}' | jq -c '{title, stale, valid_at: .valid_at[0:12]}'
{"title":"Config::load() reads GREETING from the environment","stale":false,"valid_at":"b1b1e19abf97"}
```

Editing does not clear staleness; `validate` does, by moving `valid_at` to HEAD (or to `rev`). Notes that are still correct are validated as they are.

## 13. Release and leave

```sh
$ agit lease release --all --json | jq -c '.result | length'
1
```

Through the SDKs' `AgentSession` this happens on exit even when your code raises ([9](09-sdks.md)).

## The JSON you will branch on

| Situation | `error.code` | exit | what to do |
| --- | --- | --- | --- |
| another agent moved the ref you based your update on | `ref_conflict` (`retryable: true`) | 75 | fetch/pull, re-check, retry |
| a lock was busy | `lock_timeout` (`retryable: true`) | 75 | back off, retry |
| another agent holds the path | `lease_conflict` | 3 | pick other work, wait for `lease.release` or expiry, or ask |
| three-way merge needs a decision | `merge_conflicts` | 3 | resolve the marked files, `agit add`, commit |
| local edits would be overwritten | `dirty_worktree` | 3 | checkpoint or commit first |
| server policy refused the change | `policy_violation` | 1 | read the message: missing intent/agent/signature, approvals, blockers, or a direct push to a protected branch; open a PR instead |
| non-fast-forward push | `invalid` | 1 | `agit pull` (or `fetch` + merge), push again; `--force` only when you mean it |
| nothing staged | `nothing_to_commit` | 1 | stage something or `--allow-empty` |

The complete code list is in [6. CLI reference](06-cli-reference.md#exit-codes-and-error-codes). Patterns for teams of agents (one workspace per agent, lease etiquette, reviewer agents, policy templates) are in [12. Multi-agent workflows](12-multi-agent-workflows.md).
