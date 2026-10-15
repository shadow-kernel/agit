# agit Python SDK

`agit-sdk` is a thin, typed Python client for the [agit](../../README.md) server
HTTP API (`docs/API.md`) plus a helper for the local `agit --json` CLI. It has
**no third-party runtime dependencies** (standard library `urllib` + `json`) and
supports Python 3.10+.

Import name: `agit`. Distribution name: `agit-sdk`.

## Install

From the repository root:

```sh
pip install -e sdk/python            # library only
pip install -e "sdk/python[dev]"     # plus pytest
```

## Quick start

Start a server and push a repository as in the [Server quick start](../../README.md#server-quick-start-), then:

```python
from agit import Client, AgentSession, RefUpdate, expect_is

client = Client("http://127.0.0.1:7878/api/v1", agent="coder-1", model="my-model")

print(client.health())                      # Health(status='ok', version='0.1.0')
for repo in client.list_repos():
    print(repo.name, repo.default_branch, repo.head)

main = client.refs("demo", prefix="refs/heads/main")[0]
log = client.commits("demo", rev="main", limit=10, task="DEMO-7")
ctx = client.context("demo", "src/auth/login.rs")      # notes, recent commits, leases, open PRs

# compare-and-swap ref update (atomic across all updates)
client.update_refs("demo", [RefUpdate("refs/heads/main", expect_is(main.id), new_commit_id)],
                   message="land DEMO-7")

# pull requests
pr = client.create_pull("demo", "Add x", source="refs/heads/ws/coder-1", intent="Provide x", task="DEMO-7")
client.review_pull("demo", pr.number, "approve", "lgtm")
merged = client.merge_pull("demo", pr.number)            # MergeResponse(pr=..., merged_commit=...)

# policy: protect main, require a PR with one approval
pol = client.policy("demo")
pol.protected_branches, pol.require_pull_request, pol.require_approvals = ["main"], True, 1
client.set_policy("demo", pol)                           # violations raise PolicyViolation (403)

# knowledge
note = client.create_note("demo", "x.txt is consumed by the importer",
                          tags=["kind:decision", "area:import"], paths=["x.txt"])
for s in client.notes("demo", tags=["area:import"]):
    print(s.id, s.title, s.excerpt)

# live events (Server-Sent Events)
for event in client.stream_events("demo", since=0):
    print(event.seq, event.kind, event.actor, event.data)
```

### The agent loop

`AgentSession` is the loop the main README describes: read context, claim
leases for the paths you will change, do the work (edit, `agit commit`,
`agit push` through the CLI), open a PR, record what you learned, and release
the leases when you are done. The context manager releases leases on exit even
when your code raises.

```python
from agit import Client, AgentSession

with AgentSession(Client(), "demo", agent_id="coder-1") as s:
    ctx = s.context("src/auth/login.rs")          # ContextResponse
    for note in ctx.notes:
        print(note.title, note.excerpt)

    s.claim(["src/auth/**"], ttl_secs=1800, note="DEMO-7")   # LeaseConflict if someone else holds it

    ...  # edit files, then `agit commit --intent ... --task DEMO-7` and `agit push`

    pr = s.open_pr("Add login", source="refs/heads/ws/coder-1", intent="Allow sign-in", task="DEMO-7")
    s.remember("login.rs validates the email before the password",
               tags=["area:auth", "kind:gotcha"], paths=["src/auth/**"], pulls=[pr.number])
# leases released here
```

### Local CLI (`agit --json`)

```python
from agit.cli_json import run, stream

status = run(["status"], cwd="/path/to/repo")                   # the `result` of the envelope
run(["commit", "-m", "Add x", "--intent", "Provide x"], cwd=".", agent="coder-1")
for event in stream(["events", "--follow"], cwd="."):           # one object per JSON line
    ...
```

`--json` is appended automatically. The binary is `agit` on `PATH` unless
`AGIT_BIN` points elsewhere.

## Return types

Responses are parsed into dataclasses from `agit.types` with the exact field
names of `docs/API.md` (`RepoSummary`, `RefEntry`, `CommitEntry`, `CommitDetail`,
`Change`, `DiffResponse`, `PullRequest`, `PullDetail`, `Review`, `Comment`,
`Check`, `MergeResponse`, `Note`, `NoteSummary`, `TagCount`, `TagRule`, `Policy`,
`Lease`, `Event`, `ContextResponse`, `AgentActivity`). Parsing is lenient: unknown fields
are ignored and the raw payload is kept on `.raw`. A few small responses are
plain dicts: `tree()`, `delete_repo()`, `release_lease()`, `check_lease()`,
`delete_note()`, `object_json()`. `fetch_objects()` returns the raw object
stream as `bytes`; `get_object()` returns `(kind, payload)`.

Enum values are plain strings as on the wire: PR `state` `open|merged|closed`,
review `decision` `approve|request_changes|comment`, comment `kind`
`question|suggestion|blocker|note`, check `status` `pending|success|failure`,
merge `strategy` `merge|ff`, lease `mode` `exclusive|shared`.

## Error handling

Every failure raises `agit.AgitError` (or a subclass) built from the
`{"ok": false, "error": {code, message, retryable, details}}` envelope:

| attribute | meaning |
| --- | --- |
| `.code` | stable error code (`ref_conflict`, `lease_conflict`, `merge_conflicts`, `not_found`, `already_exists`, `forbidden`, `invalid`, ...). Branch on this, never on the message. |
| `.message` | human-readable text |
| `.retryable` | `True` when the identical call may succeed if repeated (`ref_conflict`, `lock_timeout`, transport errors) |
| `.status` | HTTP status (`None` for CLI and transport errors) |
| `.details` | the `details` object when present |
| `.exit_code` | CLI exit code (`cli_json` only) |

Subclasses selected by code: `RefConflict` (`.name`, `.expected`, `.actual`),
`LeaseConflict` (`.path`, `.holder`, `.lease_id`), `MergeConflicts` (`.count`),
`NotFound` (`not_found`, `*_not_found`, `unknown_revision`, 404), `AlreadyExists`,
`Forbidden`, `PolicyViolation` (403 `policy_violation` from a protected-branch rule), `Invalid`, `TransportError` (connection refused / timeout, always retryable).

```python
from agit import AgitError, RefConflict, LeaseConflict

try:
    client.update_refs("demo", [RefUpdate("refs/heads/main", expect_is(old), new)])
except RefConflict as e:          # ref moved: re-read, reconcile, retry
    print(e.name, e.expected, e.actual, e.retryable)
except LeaseConflict as e:        # not retryable: wait, pick other work, or negotiate
    print(e.path, "held by", e.holder)
except AgitError as e:
    print(e.code, e.status, e.message)
```

## Environment variables

| variable | used by | meaning |
| --- | --- | --- |
| `AGIT_TOKEN` | `Client` | default bearer token (`Authorization: Bearer ...`) |
| `AGIT_AGENT` | `Client`, `cli_json` | default acting agent id (`X-Agit-Agent`) |
| `AGIT_MODEL` | `Client`, `cli_json` | default model name (`X-Agit-Model`) |
| `AGIT_SERVER_URL` | `Client(base_url=None)`, integration tests | server base URL including `/api/v1` |
| `AGIT_BIN` | `cli_json` | path of the `agit` binary (default: `agit` on `PATH`) |

Explicit constructor arguments always win over the environment.

## Running the tests

Unit tests run against an in-process fake server (`http.server`) and need no
Rust build:

```sh
cd sdk/python
python3 -m venv .venv && . .venv/bin/activate
pip install -e ".[dev]"
pytest -q
```

The integration test talks to a real server and is skipped unless
`AGIT_SERVER_URL` is set:

```sh
agit-server --data-dir /tmp/agit-data --listen 127.0.0.1:7878 &
AGIT_SERVER_URL=http://127.0.0.1:7878/api/v1 pytest -q -m integration
```

## Status

Covers every endpoint in `docs/API.md` v1. The SDK follows the server's
compatibility rule: additions are backwards compatible (unknown fields are
ignored), removals do not happen without a release note.

## Webhooks

```python
wh = client.create_webhook("demo", "https://ci.example/agit", kinds=["ref.*", "pull.*"], secret="s3cret")
client.test_webhook("demo", wh.id)       # queues a `ping`
client.delete_webhook("demo", wh.id)
```

Deliveries carry `X-Agit-Event`, `X-Agit-Repo`, `X-Agit-Delivery` and, with a secret,
`X-Agit-Signature: blake3=<hex>` (keyed BLAKE3 over the body, see `docs/API.md`).
