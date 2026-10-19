# 9. SDKs

Two thin, dependency-free clients wrap the HTTP API: `agit-sdk` for Python (`sdk/python`, import name `agit`, Python 3.10+) and `@agit/sdk` for TypeScript (`sdk/typescript`, ESM and CJS, Node 18+/Deno/Bun/browsers). Both expose every endpoint in [docs/API.md](../API.md) with the field names of the wire format, raise typed errors carrying the server's `code`, `retryable` and `details`, stream Server-Sent Events, and ship an `AgentSession` helper that mirrors the agent loop. Neither computes BLAKE3, so neither can build commits locally: edit, commit and push with the CLI (or MCP), and use the SDK for everything around it.

The scripts below were run against the demo server of [chapter 4](04-quickstart-agents.md); their output is shown trimmed.

## Python

### Install

```sh
pip install -e sdk/python              # or: pip install agit-sdk
pip install -e "sdk/python[dev]"       # plus pytest
```

Without installing, `PYTHONPATH=sdk/python/src python3 script.py` works.

### Client

```python
from agit import Client, AgentSession, AgitError, LeaseConflict, RefUpdate, expect_is

client = Client("http://127.0.0.1:17979/api/v1", token="handbook-demo-token", agent="coder-2", model="demo-model-2")
print("health:", client.health())
print("repos:", [(r.name, r.head[:12]) for r in client.list_repos()])
print("policy:", client.policy("demo").protected_branches, client.policy("demo").require_approvals)
```

```
health: Health(status='ok', version='0.1.0')
repos: [('demo', 'b1b1e19abf97')]
policy: ['main'] 1
```

Constructor arguments win over the environment; otherwise `AGIT_SERVER_URL`, `AGIT_TOKEN`, `AGIT_AGENT` and `AGIT_MODEL` are read. `client.with_agent("reviewer-1")` returns a copy acting as another agent.

Responses are dataclasses from `agit.types` (`RepoSummary`, `RefEntry`, `CommitEntry`, `CommitDetail`, `PullRequest`, `PullDetail`, `Note`, `NoteSummary`, `StaleNote`, `Policy`, `Lease`, `Event`, `ContextResponse`, `SignatureInfo`, `KeyEntry`, `Webhook`, `AgentActivity`, …). Parsing is lenient: unknown fields are ignored and the raw payload stays on `.raw`. Enum values are plain strings as on the wire.

### The agent loop with `AgentSession`

```python
with AgentSession(client, "demo", agent_id="coder-2") as s:
    ctx = s.context_pack("src/config.rs", budget=800)
    print("notes:", [(n.title, n.stale) for n in ctx.notes])
    print("suggested_tags:", ctx.suggested_tags, "agents_active:", ctx.agents_active)
    lease = s.claim(["src/config.rs"], ttl_secs=600, note="SDK walkthrough")
    print("lease:", lease.id, lease.holder, lease.mode)
    try:
        client.claim_lease("demo", holder="coder-9", paths=["src/**"], ttl_secs=60)
    except LeaseConflict as e:
        print("coder-9 blocked:", e.code, e.path, "held by", e.holder, "retryable:", e.retryable)
    # ... edit files, then `agit commit --intent ... --task ...` and `agit push` with the CLI ...
    note = s.remember("Config is read once at startup", body="Config::load() is called from main(); there is no reload.",
                      tags=["kind:fact", "area:core"], paths=["src/main.rs", "src/config.rs"])
    print("note:", note.id, note.valid_at[:12], note.stale)
    print("stale now:", [n.title for n in s.stale_notes()])
    print("pulls merged:", [(p.number, p.state) for p in client.pulls("demo", state="merged")])
print("leases after exit:", [l.holder for l in client.leases("demo")])
```

```
notes: [('Greeting text lives in Config, not main.rs', False), ('PR #1: greeting moved into Config', False), ('Config::load() reads GREETING from the environment', False)]
suggested_tags: ['area:core', 'lang:rust'] agents_active: ['coder-1', 'reviewer-1']
lease: 9a11e4546ded coder-2 exclusive
coder-9 blocked: lease_conflict src/** held by coder-2 retryable: False
note: n_15761d1fc397 b1b1e19abf97 False
stale now: []
pulls merged: [(4, 'merged'), (1, 'merged')]
leases after exit: []
```

`AgentSession` methods: `context(path, limit)`, `context_pack(path, budget, limit)`, `claim(paths, ttl_secs, note, shared)`, `release()` (also on exit, even when the block raises), `open_pr(title, source, target, body, intent, task)`, `remember(title, body, tags, paths, commits, pulls, confidence, valid_at)`, `stale_notes(rev, drafts)`, `validate_note(note_or_id, rev)`, `accept_note(draft_or_id, **patch)`, `events(since, limit)`, `follow(since)`.

Opening and merging a PR from the SDK (the branch must already be pushed):

```python
pr = s.open_pr("Add login", source="refs/heads/ws/coder-1", intent="Allow sign-in", task="DEMO-7")
reviewer = client.with_agent("reviewer-1")
reviewer.comment_pull("demo", pr.number, kind="suggestion", path="src/auth/login.rs", line=12, body="…")
reviewer.set_check("demo", pr.number, "tests", "success")
reviewer.review_pull("demo", pr.number, "approve", "LGTM")
merged = reviewer.merge_pull("demo", pr.number)          # MergeResponse(pr=..., merged_commit=...)
```

### Errors and compare-and-swap

```python
main = [r for r in client.refs("demo", prefix="refs/heads/") if r.name == "refs/heads/main"][0]
try:
    client.update_refs("demo", [RefUpdate("refs/heads/sdk-tmp", expect_is("0" * 63 + "1"), main.id)])
except AgitError as e:
    print("ref update failed:", e.code, e.status, e.retryable, e.details)
for ev in client.events("demo", since=0, limit=3):
    print("event:", ev.seq, ev.kind, ev.actor)
```

```
ref update failed: ref_conflict 409 True {'name': 'refs/heads/sdk-tmp', 'expected': '0000…0001', 'actual': None}
event: 119 lease.claim coder-2
event: 120 note.create coder-2
event: 121 lease.release coder-2
```

Every failure raises `agit.AgitError` with `.code`, `.message`, `.retryable`, `.status` (`None` for CLI and transport errors) and `.details`. Subclasses: `RefConflict` (`.name`, `.expected`, `.actual`), `LeaseConflict` (`.path`, `.holder`, `.lease_id`), `MergeConflicts` (`.count`), `NotFound`, `AlreadyExists`, `Forbidden`, `PolicyViolation`, `Invalid`, `TransportError` (always retryable). `expect_is`, `expect_any`, `expect_absent` build the CAS expectations.

### Streaming events and the local CLI

```python
for event in client.stream_events("demo", since=0):      # SSE, replay then live
    print(event.seq, event.kind, event.actor, event.data)

from agit.cli_json import run, stream                      # the `agit --json` envelope, unwrapped
status = run(["status"], cwd="/path/to/repo")
run(["commit", "-m", "Add x", "--intent", "Provide x"], cwd=".", agent="coder-1")
for event in stream(["events", "--follow"], cwd="."):
    ...
```

`cli_json` appends `--json`, raises `AgitError` with `.exit_code` on failure, and uses `AGIT_BIN` or `agit` on `PATH`. Knowledge v2 helpers (`notes(q=…, tags=…, path=…, commit=…, pull=…, drafts=…)`, `stale_notes`, `validate_note`, `accept_note`, `context_pack`), keys (`register_key`, `keys`, `delete_key`), webhooks (`create_webhook`, `test_webhook`, `delete_webhook`) and the policy (`policy`, `set_policy`) are documented with examples in [sdk/python/README.md](../../sdk/python/README.md).

### Tests

```sh
cd sdk/python && python3 -m venv .venv && . .venv/bin/activate && pip install -e ".[dev]" && pytest -q
AGIT_SERVER_URL=http://127.0.0.1:7878/api/v1 pytest -q -m integration     # against a real server
```

## TypeScript

### Install

```sh
pnpm add @agit/sdk        # or npm install / yarn add
cd sdk/typescript && pnpm install && pnpm build && pnpm test   # from the repository
```

Needs only a WHATWG `fetch`. ESM (`dist/index.js`) and CJS (`dist/index.cjs`) builds with `.d.ts` types.

### Client and `AgentSession`

```ts
import { AgitClient, AgentSession, LeaseConflictError, RefConflictError, expectIs, isRetryable } from "@agit/sdk";

const client = new AgitClient({ baseUrl: "http://127.0.0.1:17979/api/v1", token: "handbook-demo-token", agent: "coder-3", model: "demo-model-3" });
console.log("health:", await client.health());
const me = new AgentSession(client, "demo", "coder-3");
const ctx = await me.contextPack("src/main.rs", 800);
console.log("notes:", ctx.notes.map((n) => [n.title, n.stale]));
console.log("agents_active:", ctx.agents_active, "files:", (ctx.files ?? []).map((f) => [f.path, f.truncated]));
await me.withLeases(["src/main.rs"], async (lease) => {
  console.log("holding", lease.id, "as", lease.holder, "until", new Date(lease.expires_ms).toISOString());
  try { await client.leases.claim("demo", { holder: "coder-9", paths: ["src/**"], ttl_secs: 60 }); }
  catch (e) { if (e instanceof LeaseConflictError) console.log("coder-9 blocked:", e.code, e.details); else throw e; }
}, { ttlSecs: 600, note: "TS walkthrough" });
console.log("leases after withLeases:", (await client.listLeases("demo")).map((l) => l.holder));
```

```
health: { status: 'ok', version: '0.1.0' }
notes: [ [ 'Greeting text lives in Config, not main.rs', false ], [ 'PR #1: greeting moved into Config', false ] ]
agents_active: [ 'Dana Developer', 'coder-1', 'reviewer-1' ] files: []
holding 0799f461ab5e as coder-3 until 2026-10-15T19:48:08.601Z
coder-9 blocked: lease_conflict { path: 'src/**', holder: 'coder-3', lease_id: '0799f461ab5e' }
leases after withLeases: []
```

`withLeases(paths, fn, {ttlSecs, note, shared})` releases the lease when `fn` returns or throws. The session also offers `contextPack`, `openPullRequest({title, source, target, body, intent, task})` (a bare branch name expands to `refs/heads/...`), `remember({...})`, `staleNotes(rev)`, `validateNote(noteOrId, rev)`, `acceptNote(draftOrId, patch)`.

### Reviewing, merging, CAS and errors

```ts
const [main] = (await client.listRefs("demo", "refs/heads/")).filter((r) => r.name === "refs/heads/main");
try {
  await client.updateRefs("demo", [{ name: "refs/heads/ts-tmp", expect: expectIs("0".repeat(63) + "1"), new: main.id }]);
} catch (e) {
  if (e instanceof RefConflictError) console.log("ref conflict:", e.status, e.retryable, isRetryable(e), e.details);
}
const reviewer = client.withAgent("reviewer-2");
const detail = await reviewer.getPull("demo", 4);
console.log("PR 4:", detail.pr.state, detail.pr.reviews.map((r) => r.decision), detail.changes.map((c) => c.path));
const hits = await client.listNotes("demo", { q: "greeting", limit: 2 });
console.log("search:", hits.map((h) => [h.title, h.score?.toFixed(2)]));
```

```
ref conflict: 409 true true { name: 'refs/heads/ts-tmp', expected: '0000…0001', actual: null }
PR 4: merged [ 'approve' ] [ 'src/config.rs' ]
search: [ [ 'Config::load() reads GREETING from the environment', '0.19' ], [ 'PR #1: greeting moved into Config', '0.18' ] ]
```

Review and merge: `reviewer.commentPull(repo, n, {path, line, kind, body})`, `reviewer.setCheck(repo, n, {name, status})`, `reviewer.reviewPull(repo, n, {decision, body})`, `reviewer.mergePull(repo, n, {strategy})`. Resource groups exist as `client.pulls.*`, `client.leases.*`, `client.knowledge.*`, `client.keys.*`.

Errors are `AgitError` with `code`, `message`, `status`, `retryable`, `details`; subclasses `RefConflictError`, `LeaseConflictError`, `MergeConflictsError`, `PolicyViolationError`, `NotFoundError`. Transport failures have `status: 0` and code `network` or `timeout` (per-request `timeoutMs`, default 30 s), both retryable; `isRetryable(e)` covers all of them.

### Streaming

```ts
const ctl = new AbortController();
let n = 0;
for await (const ev of client.streamEvents("demo", { since: 0, signal: ctl.signal })) {
  console.log("event:", ev.seq, ev.kind, ev.actor ?? "-");
  if (++n >= 3) ctl.abort();
}
```

```
event: 1 repo.create -
event: 2 ref.update anonymous
event: 3 commit anonymous
```

`streamEvents` replays after `since`, stays live, reconnects from the last seen `seq` with backoff (`retryMs`, doubling to 30 s, honouring the server's `retry:`), and throws non-retryable server errors. Events already buffered when you abort may still be yielded. Polling: `client.listEvents(repo, {since, limit})`.

### Objects

`encodeObjectStream`/`decodeObjectStream` implement the frame format for `uploadObjects`, `fetchObjects` and `POST /fetch`. Object ids come from the server (BLAKE3 is not bundled).

## Authentication from an SDK

Both SDKs speak the same auth surface as the CLI ([chapter 13](13-server-administration.md#authentication)): `whoami`, `challenge` + `createSession` / `create_session`, the convenience `loginWithKey` / `login_with_key` that takes a signing callback, `logout`, and the admin calls for tokens and authorized keys. Neither bundles an Ed25519 implementation: pass a signing function from `node:crypto` or `cryptography`, or take the token from `agit auth login --print-token`. A missing or invalid token is `401 unauthorized` (Python `Unauthorized`; TypeScript `code: "unauthorized"`), a scope problem is `403 forbidden`.

```python
created = client.login_with_key(pub_line, key.sign)        # cryptography's Ed25519PrivateKey.sign
client.add_authorized_key(pub_line, [Scope("demo", "write")], agent="coder-2")
```

```ts
await client.loginWithKey({ publicKey: pubLine, sign: (m) => sign(null, m, key) });   // node:crypto
await client.auth.tokens.create({ name: "ci", scopes: [{ repo: "demo", access: "read" }] });
```

## Choosing between CLI, MCP and SDK

| Need | Use |
| --- | --- |
| commit, checkpoint, switch, merge, local leases | CLI (`agit … --json`) or MCP tools |
| an orchestrator that watches many agents and repositories | SDK: events stream, `/agents`, PR and policy calls |
| a reviewer or CI agent that never touches a working tree | SDK or raw HTTP: `getPull`, `setCheck`, `reviewPull`, `mergePull` |
| a model with tool calling inside one repository | MCP (`agit mcp`) |
| a quick script | `agit.cli_json.run([...])` in Python |

Both SDKs follow the server's compatibility rule: additions are backwards compatible (unknown fields are ignored), removals do not happen without a release note.
