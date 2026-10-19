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
ctx = client.context("demo", "src/auth/login.rs")      # notes, recent commits, leases, open PRs, active agents

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
for s in client.notes("demo", q="importer", tags=["area:import"]):   # BM25-ranked, best first
    print(s.id, s.title, s.excerpt, s.score, s.stale)

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
    ctx = s.context_pack("src/auth/login.rs", budget=6000)   # ContextResponse incl. file texts
    for note in ctx.notes:                         # fresh notes first, then stale ones
        print(note.title, note.excerpt, "(stale)" if note.stale else "")
    for f in ctx.files:                            # the file plus up to 5 siblings, within the budget
        print(f.path, len(f.text), f.truncated)
    print("also working here:", ctx.agents_active)

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
`Check`, `MergeResponse`, `Note`, `NoteSummary`, `StaleNote`, `TagCount`, `TagRule`, `Policy`,
`Lease`, `Event`, `ContextResponse`, `ContextFile`, `SignatureInfo`, `KeyEntry`, `Webhook`,
`AgentActivity`). Parsing is lenient: unknown fields
are ignored and the raw payload is kept on `.raw`. A few small responses are
plain dicts: `tree()`, `delete_repo()`, `release_lease()`, `check_lease()`,
`delete_note()`, `delete_key()`, `delete_webhook()`, `test_webhook()`, `object_json()`. `fetch_objects()` returns the raw object
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
`Webhook.has_secret` tells whether a secret is set; the secret itself is never returned.

## Knowledge v2: search, validity, drafts, context packs

Notes carry `valid_at` (the commit they were written or last validated against,
default HEAD at creation), `stale` (computed on every read: a covered path changed
between `valid_at` and HEAD) and `draft` (machine-generated, hidden until accepted).
Summaries also carry `score` for ranked searches.

```python
# ranked full-text search (BM25 over title, body, tags, paths); filters are post-filters
hits = client.notes("demo", q="session store", path="src/auth/login.rs", limit=5)
for h in hits:
    print(h.score, h.title, "stale" if h.stale else "fresh")
client.notes("demo", commit="main", pull=3)                  # notes linking a commit / PR
client.notes("demo", drafts=True, include_superseded=True)   # everything

# validity
client.create_note("demo", "Pinned", valid_at=commit_id)      # 412 object_not_found if unknown
for s in client.stale_notes("demo", rev="HEAD"):              # StaleNote: summary + changed_paths
    print(s.id, s.changed_paths)
    client.validate_note("demo", s.id)                        # "still true at HEAD" -> valid_at = HEAD
    # ...or fix the text: client.update_note("demo", s.id, body="...") then validate

# drafts: every merged PR leaves a draft note (event `note.draft`)
for d in client.notes("demo", drafts=True, pull=pr.number):
    client.accept_note("demo", d.id, title="Cleaner title", tags=["kind:change", "area:auth"])
    # or client.delete_note("demo", d.id) to discard it

# context pack: context plus file texts, kept within ~budget tokens
pack = client.context_pack("demo", "src/auth/login.rs", budget=6000)
pack.agents_active        # agents holding a lease on the path or who committed to it in 24 h
pack.fresh_notes, pack.stale_notes
[(f.path, f.truncated) for f in pack.files]

# commits link back to notes
detail = client.commit("demo", "main")
[n.title for n in detail.notes]                               # up to 10 related notes
```

Through `AgentSession`: `s.context_pack(path, budget)`, `s.stale_notes(rev)`,
`s.validate_note(note_or_id, rev)`, `s.accept_note(draft_or_id, **patch)` and
`s.remember(..., valid_at=...)`.

## Authentication: SSH keys, sessions and tokens

The server authenticates people with SSH keys and agents with scoped tokens
(see `docs/SECURITY.md`). The SDK bundles no signing code, so the simplest way
to a token is the CLI: `agit auth login --url <server> --print-token`, then
`Client(token=...)` or `AGIT_TOKEN`. With a signing library of your choice the
SDK can do the challenge/response itself:

```python
from cryptography.hazmat.primitives.serialization import load_ssh_private_key
key = load_ssh_private_key(open("/home/me/.ssh/id_ed25519", "rb").read(), password=None)
pub = open("/home/me/.ssh/id_ed25519.pub").read().strip()
created = client.login_with_key(pub, key.sign, ttl_secs=8 * 3600)   # signs "agit-auth-v1\n<nonce>"
me = client.whoami()                      # WhoAmI(principal="tok_…", agent=…, scopes=[Scope("shop", "write")], key="key_…")
me.holds("shop", "write")                 # True
client.logout()                           # revokes the session
```

Admins manage who may sign in and mint tokens for agents:

```python
client.add_authorized_key(pub, [Scope("shop", "write")], name="dana laptop", agent="dana")
client.authorized_keys()                  # [AuthorizedKey(fingerprint="SHA256:…", scopes=[…]), …]
tok = client.create_token("ci-deploy", [{"repo": "shop", "access": "write"}], agent="ci-bot", ttl_secs=30 * 86400)
tok.token                                 # shown once
client.delete_token(tok.id); client.delete_authorized_key("key_…")
```

Errors: `Unauthorized` (401, no usable credentials) versus `Forbidden` (403,
valid credentials without the needed scope, or an SSH key that is not
authorized).

## Signing keys

Agents sign commits with Ed25519 (`agit key generate`, `agit commit --sign`); the
public key travels inside the commit. The per-repository key registry decides which
keys are trusted for which agent, and `Policy.require_signed` makes every commit
reaching a protected branch need a valid signature from a registered key.

```python
key = client.register_key("demo", "coder-1", public_key_hex)   # KeyEntry(agent, key_id, public_key, added_ms, added_by)
client.keys("demo")                                            # 409 AlreadyExists if the key belongs to another agent
client.delete_key("demo", key.key_id)                          # {"deleted": key_id}

sig = client.commit("demo", "main").signature                  # SignatureInfo or None
sig.status            # "unsigned" | "valid" | "invalid"
sig.key_id, sig.signer, sig.public_key   # for "valid"
sig.reason            # for "invalid"
sig.trusted           # key registered for the signer
sig.is_trusted        # valid and trusted

pol = client.policy("demo")
pol.protected_branches, pol.require_signed = ["main"], True
client.set_policy("demo", pol)
```
