# 12. Multi-agent workflows

Patterns for running several agents against one repository, with the agit primitives that make each pattern safe. Everything here uses commands and codes verified in the earlier chapters; policy JSON is accepted as shown by `PUT /repos/{repo}/policy`.

## One workspace per agent

Never let two agents share a working directory and index. Give each one a workspace (shared `.agit`, separate tree and HEAD, own branch `ws/<name>`), or a separate clone when they run on different machines.

```sh
agit ws new coder-1 --agent coder-1                      # ../repo.coder-1 on branch ws/coder-1
agit ws new coder-2 --agent coder-2 --path /work/coder-2 --from main
agit ws list --json
agit ws remove coder-2 --delete                          # when the agent is retired; its branch stays
```

Why it works: commits in one workspace are immediately visible to `log` in the others, leases claimed from any workspace bind all of them, the event log is shared, and a crashed agent's workspace can be deleted without touching anyone else's state. Start an agent's process inside its workspace directory (`cd` or `agit -C`), and set `AGIT_AGENT` to the workspace name so events, leases and commits line up.

Integration happens through `main`: an agent pushes `ws/coder-1` and opens a PR, or, locally without a server, a human (or an integrator agent) merges `ws/coder-1` into `main` in the primary workspace. Rebase does not exist in 0.1.0; a workspace branch catches up with `agit merge main`.

## Lease etiquette

Leases are advisory, so their value depends on everyone following the same rules:

1. **Claim before the first edit, not before the commit.** `agit know context <path>` tells you who else is active; then `agit lease claim <dir> --ttl 30m --note "<task id>: <what>"`.
2. **Claim directories, not files**, for anything larger than a one-line fix. One lease on `src/auth` is cheaper and clearer than fifty on files, and covering leases satisfy `lease check`. The Claude Code post-edit hook claims per file only when no covering lease exists.
3. **Short TTLs, renewed while working.** 30 minutes is a good default; renew on a timer or after each tool call (`agit lease renew <id> --ttl 30m`). A crashed agent then blocks nobody for more than one TTL.
4. **Always put a note** on the lease. Other agents and the lease board show it, and it turns "src/auth is held by coder-2" into "coder-2 is rotating session tokens for SEC-12, 22 min left".
5. **Shared leases for reading.** `--shared` announces "my work depends on this" without blocking; an exclusive claimant can see the readers and warn them.
6. **Never `--force` through someone else's lease** unless a human says so. The force is recorded in the event; treat it as an incident, not a shortcut.
7. **Release on exit**: `agit lease release --all` (or `AgentSession`/`withLeases` in the SDKs). Expiry is the fallback, not the plan.
8. **When blocked** (`lease_conflict`): pick other work from the queue, or wait for `lease.release` of that id on the event stream (`agit events --follow --kind lease.`), or until `expires_ms`; there is no expiry event. Do not poll `lease list` in a tight loop.

Local leases (`.agit/leases/`) coordinate agents sharing a filesystem; server leases (`POST /repos/{repo}/leases`) coordinate agents on different machines. Use one or the other consistently per deployment; the Claude Code hooks use local leases, the SDKs' `AgentSession` uses server leases.

## Checkpoint cadence

Checkpoints are cheap (one tree walk plus the changed blobs) and invisible in history, so err on the side of too many:

- before any destructive step (rewriting a file, running a formatter, applying a patch);
- at the end of every turn or tool batch (`--if-changed` makes this idempotent);
- before `restore`, `reset --hard`, `switch --force`, `merge`.

```sh
agit checkpoint -m "before applying patch 3" --if-changed
agit checkpoint list -n 5
agit restore <checkpoint id>          # whole tree back, untracked files included
```

Recover with `agit restore <id>`; promote a good checkpoint with an ordinary `add` and `commit`. `agit gc` expires checkpoints after 30 days (`--checkpoint-expire`). Checkpoints belong to one workspace and are written with `Expect::Any`, so there is no contention.

## Commit discipline

- Small, frequent commits, each with `--intent` (why) and `--task` (ticket). Policies can require both; the Claude Code guard refuses `agit commit` without `--intent`.
- One `AGIT_AGENT` per role, one `AGIT_RUN_ID` per run; `--meta` for tool data (`--meta ci.tests=passed --meta tool.version=…`).
- Sign when the repository asks for it: `agit key generate --register` once per agent and machine, then signing is automatic.
- On `lease_conflict` at commit time: do not force; stash or checkpoint, and raise it.

## PR-based integration

The default integration path with a server:

```sh
agit pr create --push -t "<title>" --intent "<why>" --task <id>
agit pr view <n> --json            # mergeable, conflicts, reviews, checks
```

A PR that is not mergeable (`conflicts` non-empty) is fixed by the author: `agit pull`/`agit merge main` on the branch, resolve, push again; the PR follows the branch head. Merging is a server operation with a compare-and-swap on the target, so two PRs racing for `main` produce one merge and one `ref_conflict`; the loser re-checks mergeability and retries.

Strategy: `merge` (default) creates a merge commit authored by the merging agent; `ff` fast-forwards when possible and fails otherwise. Squash and rebase strategies are 📋 (RFC 0005).

## Reviewer agents

A reviewer agent never needs a working tree. Through the SDK or HTTP it reads `GET /pulls/{n}` (commits, changes, mergeability), `GET /pulls/{n}/diff`, the context packs of the changed paths (notes, especially `kind:gotcha` and `kind:spec`), and then writes:

- `comment` with `kind: blocker` for anything that must change (blocks the merge while unresolved when `block_on_blockers` is on), `suggestion` for optional improvements, `question` for clarification, `note` for the record; anchor to `path` and `line`;
- `review` with `request_changes` or `approve` (the latest decision per reviewer counts; the author cannot approve their own PR);
- `checks` from a CI agent: `{name, status: pending|success|failure, url}`; with `require_checks` all must be `success`.

Blockers are resolved by whoever fixes them (`POST /pulls/{n}/comments/{id}/resolve`), after which the reviewer re-reviews. A merge attempt that is refused tells the reviewer exactly why:

```json
{"code":"policy_violation","message":"policy violation: policy violation: a reviewer requested changes"}
{"code":"policy_violation","message":"policy violation: unresolved blocker comments"}
{"code":"policy_violation","message":"policy violation: 0 approval(s), 1 required"}
```

Make the reviewer leave knowledge too: a `kind:decision` note for the reasoning behind an approval of a contentious change, or an accepted and edited draft note after the merge.

## Policy templates

**Solo developer with agents** (keep history honest, do not slow yourself down):

```json
{"protected_branches":["main"],"require_pull_request":false,"require_approvals":0,
 "require_intent":true,"require_agent":false,"block_on_blockers":true,"max_files_per_commit":50}
```

Direct pushes to `main` stay possible, but every commit must say why, and a runaway agent cannot land a 500-file commit.

**Small team, agents as contributors** (agents open PRs, humans or reviewer agents merge):

```json
{"protected_branches":["main","release/*"],"require_pull_request":true,"require_approvals":1,
 "require_intent":true,"require_agent":true,"require_checks":true,"block_on_blockers":true,
 "allowed_merge_agents":["reviewer-1","alice","bob"],"max_files_per_commit":0}
```

**Strict** (regulated or high-blast-radius code):

```json
{"protected_branches":["main","release/*"],"require_pull_request":true,"require_approvals":2,
 "require_intent":true,"require_agent":true,"require_checks":true,"require_signed":true,
 "block_on_blockers":true,"allowed_merge_agents":["release-bot"],"allowed_push_agents":[],
 "max_files_per_commit":20}
```

With `require_signed`, every agent needs a registered key (`agit key generate --register`), and the key registry becomes part of onboarding. Pair the strict policy with scoped tokens bound to agent ids ([13](13-server-administration.md#scoped-tokens)) so the identity the policy keys on is at least tied to a credential.

Policies apply to protected branches only. Workspace branches (`ws/*`) and feature branches stay unrestricted so agents can push freely while iterating.

## Handling the three conflicts

### `ref_conflict` (retryable, exit 75)

Your view of a ref is stale. Local: another process committed to the same branch between your read and write (rare with one workspace per agent). Remote: someone pushed or merged first.

```sh
agit pull            # or: agit fetch && agit merge origin/main
# resolve if needed, then
agit push            # or retry the PR merge
```

Retry with backoff; `details.actual` is the current value. Non-fast-forward pushes are detected client-side as `invalid` with the same remedy.

### `lease_conflict` (not retryable, exit 3)

Someone else owns the path. The same command will keep failing until the lease is released or expires. Options, in order: take the next task; subscribe to `lease.release` for that id; ask (a PR comment or a note); escalate to a human who may `--force` with a recorded reason.

### `policy_violation` (not retryable, exit 1)

Read the message, it names the rule. Direct push to a protected branch: open a PR. Missing intent or agent: amend by making a new commit with the fields (history rewriting is not available; `reset --soft` and recommit on an unpushed branch). Missing approvals or unresolved blockers: wait for the reviewer. Unsigned: generate and register a key, recommit.

### `merge_conflicts` (exit 3)

The tree is left with diff3 markers and the index marks the paths as conflicted; `status` shows "Merge in progress". Resolve, `agit add`, commit. `agit merge --abort` returns to the pre-merge state. Conflict kinds: `content` (text), `add_add` (both added), `delete_modify`, `mode`, `binary` (ours kept; pick a side by hand).

## Recovering

| Situation | Recovery |
| --- | --- |
| a bad commit on an unpushed branch | `agit reset --soft HEAD~1`, fix, recommit; or `agit revert <id>` to keep history |
| a bad `reset --hard` | `agit reflog`, then `agit reset --hard <previous id>` (copy the id; `HEAD@{n}` is not resolvable) |
| a destroyed working tree | `agit checkpoint list`, `agit restore <id>`; or `agit restore HEAD` to the last commit |
| a merge that went wrong before committing | `agit merge --abort` |
| an agent crashed mid-task | its leases expire by TTL (or `agit lease release --all --holder <id>`); its workspace can be removed; its checkpoints are on `refs/checkpoints/<ws>` for forensics |
| a stale clone | `agit pull`; with a submodule in the tree see the [FAQ](15-faq.md#known-discrepancies-and-rough-edges) |
| suspected corruption | `agit fsck`; objects are verified on every read, so corruption surfaces as `hash_mismatch`/`corrupt_object` rather than bad data |

## Observing the fleet

- `agit events --follow --json` locally, `GET /events/stream` or webhooks on the server: one line per commit, lease, PR action, note and policy change, with `actor`.
- `GET /agents`: per agent id, commits, last seen, open PRs, leases.
- The web UI's Agents tab (live feed), Leases board (countdowns), Pull requests and Knowledge pages.
- `agit log --agent <id>`, `--task <id>`, `--path <p>` for provenance after the fact.

For the Claude Code case, where the model is the agent, the hooks in `integrations/claude-code/` implement the lease check, context injection, automatic leases, checkpoints and the intent guard deterministically ([7](07-mcp-reference.md#claude-code-integration)).
