# @agit/sdk

TypeScript client for the [agit](https://github.com/shadow-kernel/agit) server HTTP API
(`docs/API.md`): repositories, compare-and-swap refs, objects, pull requests and reviews,
leases, knowledge notes (ranked search, validity, drafts, context packs), policies, signing
keys, webhooks and the live event stream. Built for AI agents and the
orchestrators that run them.

- Zero runtime dependencies. Needs only a WHATWG `fetch`: Node 18+, Deno, Bun, browsers.
- ESM and CommonJS builds, strict TypeScript types for every object in the API.
- Errors carry the server's stable `code`, `retryable` flag and `details`, as typed subclasses.
- Server-Sent Events are parsed over `fetch` + `ReadableStream`, so the stream carries
  `Authorization`/`X-Agit-*` headers and needs no `EventSource`.

## Install

```sh
pnpm add @agit/sdk     # or npm install @agit/sdk / yarn add @agit/sdk
```

Start a server to talk to, e.g. `agit serve` (defaults to `http://127.0.0.1:7878`).

## Quick start

```ts
import { AgitClient, AgentSession, LeaseConflictError } from "@agit/sdk";

const client = new AgitClient({
  baseUrl: "http://127.0.0.1:7878/api/v1", // default
  token: process.env.AGIT_TOKEN,            // sent as Authorization: Bearer
  agent: "coder-1",                         // sent as X-Agit-Agent
  model: "my-model-v3",                     // sent as X-Agit-Model
});

const me = new AgentSession(client, "demo", "coder-1");

// 1. Read before you touch: notes (fresh first, then stale), suggested tags, recent commits,
//    leases, open PRs, the agents active on the path and, with contextPack, the file texts.
const ctx = await me.contextPack("src/auth/login.rs", 6000); // budget in approximate tokens
console.log(ctx.notes.map((n) => [n.title, n.stale]), ctx.suggested_tags, ctx.agents_active);
for (const f of ctx.files ?? []) console.log(f.path, f.text.length, f.truncated);

// 2. Claim what you will change; the lease is released even if the work throws.
try {
  await me.withLeases(["src/auth/**"], async (lease) => {
    // ... edit, commit and push with the agit CLI in your workspace ...
    console.log("holding", lease.id, "until", lease.expires_ms);
  }, { ttlSecs: 1800, note: "rotate session tokens" });
} catch (e) {
  if (e instanceof LeaseConflictError) {
    console.log(`busy: ${e.details?.holder} holds ${e.details?.path}`); // pick other work
  } else throw e;
}

// 3. Open a pull request from your branch.
const pr = await me.openPullRequest({
  title: "Rotate session tokens on login",
  source: "ws/coder-1",              // expands to refs/heads/ws/coder-1
  intent: "Close the fixation window reported in SEC-12",
  task: "SEC-12",
});

// 4. Record what you learned for the next agent.
await me.remember({
  title: "Session store is eventually consistent",
  body: "Reads after a token rotation can lag by up to 200 ms; retry once.",
  tags: ["kind:gotcha", "area:auth"],
  paths: ["src/auth/**"],
  pulls: [pr.number],
});
```

### Reviewing and merging

```ts
const reviewer = client.withAgent("reviewer-1");
const detail = await reviewer.getPull("demo", pr.number);  // commits, changes, mergeable, conflicts
await reviewer.commentPull("demo", pr.number, { path: "src/auth/login.rs", line: 12, kind: "suggestion", body: "..." });
await reviewer.setCheck("demo", pr.number, { name: "tests", status: "success" });
await reviewer.reviewPull("demo", pr.number, { decision: "approve", body: "LGTM" });
const { merged_commit } = await reviewer.mergePull("demo", pr.number, { strategy: "merge" });
```

Resource groups are also available as `client.pulls.*`, `client.leases.*`,
`client.knowledge.*` and `client.keys.*` (`client.pulls.merge(...)`, `client.knowledge.listTags(...)`).

### Knowledge v2: search, validity, drafts, context packs

Notes carry `valid_at` (the commit they were written or last validated against, HEAD at
creation by default), `stale` (computed on every read: a covered path changed between
`valid_at` and HEAD) and `draft` (machine-generated, hidden until accepted; omitted when
false on full notes). Summaries also carry `score` for ranked searches.

```ts
// ranked full-text search (BM25 over title, tags, body, paths); other filters apply in rank order
const hits = await client.listNotes("demo", { q: "session store", path: "src/auth/login.rs", limit: 5 });
hits.map((h) => [h.score, h.title, h.stale]);
await client.listNotes("demo", { commit: "main", pull: 3 });                  // notes linking a commit / PR
await client.listNotes("demo", { drafts: true, include_superseded: true });   // everything

// validity
await client.createNote("demo", { title: "Pinned", valid_at: commitId });     // 412 object_not_found if unknown
for (const s of await client.listStaleNotes("demo", { rev: "HEAD" })) {       // StaleNote: summary + changed_paths
  console.log(s.id, s.changed_paths);
  await client.validateNote("demo", s.id);                                    // "still true at HEAD" -> valid_at = HEAD
  // ...or fix the text first: client.updateNote("demo", s.id, { body }) then validate
}

// drafts: every merged PR leaves a draft note (event `note.draft`)
for (const d of await client.listNotes("demo", { drafts: true, pull: pr.number })) {
  await client.acceptNote("demo", d.id, { title: "Cleaner title", tags: ["kind:change", "area:auth"] });
  // or client.deleteNote("demo", d.id) to discard it
}

// context pack: context plus file texts, kept within ~budget tokens
const pack = await client.getContextPack("demo", "src/auth/login.rs", 6000);
pack.agents_active;        // agents holding a lease on the path or who committed to it in 24 h
pack.files;                // [{ path, text, truncated }], undefined without include=files
await client.getContext("demo", "src/auth/login.rs", { limit: 5, include_files: true, budget: 2000 });

// commits link back to notes
const detail = await client.getCommit("demo", "main");
detail.notes.map((n) => n.title);                                            // up to 10 related notes
```

Through `AgentSession`: `me.contextPack(path, budget)`, `me.staleNotes(rev)`,
`me.validateNote(noteOrId, rev)`, `me.acceptNote(draftOrId, patch)` and
`me.remember({ ..., validAt })`.

### Signing keys

Agents sign commits with Ed25519 (`agit key generate`, `agit commit --sign`); the public key
travels inside the commit. The per-repository key registry decides which keys are trusted for
which agent, and `Policy.require_signed` makes every commit reaching a protected branch need a
valid signature from a registered key.

```ts
const key = await client.registerKey("demo", { agent: "coder-1", public_key: publicKeyHex });
// KeyEntry { agent, key_id, public_key, added_ms, added_by }; 409 already_exists if the key belongs to another agent
await client.listKeys("demo");
await client.deleteKey("demo", key.key_id);                                  // { deleted: key_id }

const { signature } = await client.getCommit("demo", "main");                // SignatureInfo | undefined
switch (signature?.status) {
  case "valid":    console.log(signature.signer, signature.key_id, signature.trusted); break;
  case "invalid":  console.log(signature.reason); break;
  case "unsigned": break;
}

const policy = await client.getPolicy("demo");
await client.setPolicy("demo", { ...policy, protected_branches: ["main"], require_signed: true });
```

### Webhooks

```ts
const wh = await client.createWebhook("demo", { url: "https://ci.example/agit", kinds: ["ref.*", "pull.*"], secret: "s3cret" });
wh.has_secret;                                   // the secret itself is never returned
await client.testWebhook("demo", wh.id);         // queues a `ping`
await client.deleteWebhook("demo", wh.id);
```

Deliveries carry `X-Agit-Event`, `X-Agit-Repo`, `X-Agit-Delivery`, `X-Agit-Webhook` and, with
a secret, `X-Agit-Signature: blake3=<hex>` (keyed BLAKE3 over the body, see `docs/API.md`).

### Compare-and-swap refs

```ts
import { expectIs, expectAbsent, expectAny, RefConflictError } from "@agit/sdk";

const [main] = await client.listRefs("demo", "refs/heads/main");
try {
  await client.updateRefs("demo", [
    { name: "refs/heads/main", expect: expectIs(main.id), new: newCommitId },
    { name: "refs/heads/tmp", expect: expectAny(), new: null }, // delete
  ], "promote tmp");
} catch (e) {
  if (e instanceof RefConflictError) {
    // someone moved main first: e.details = { name, expected, actual }; re-read and retry
  }
}
```

## Streaming events

`streamEvents` is an `AsyncIterable` over `GET /repos/{repo}/events/stream`. It replays
events after `since`, then stays live, and reconnects from the last seen `seq` when the
connection drops. Stop by breaking out of the loop or aborting the signal.

```ts
const ctl = new AbortController();
for await (const ev of client.streamEvents("demo", { since: 0, signal: ctl.signal })) {
  if (ev.kind === "pull.open") console.log("new PR", ev.data);
  if (ev.kind === "lease.claim" && ev.actor !== "coder-1") console.log("someone claimed", ev.data);
}
```

Options: `since`, `signal`, `reconnect` (default `true`), `retryMs` (initial backoff,
doubles to 30 s, honours the server's `retry:`), `maxRetries`. Non-retryable server errors
(unknown repository, bad token) are thrown. Polling is available too:
`client.listEvents(repo, { since, limit })`.

## Error handling

Every failure is an `AgitError` with `code`, `message`, `status`, `retryable` and `details`.
Subclasses let you `instanceof` the cases agents usually handle:

| class | code | status | what to do |
| --- | --- | --- | --- |
| `RefConflictError` | `ref_conflict` | 409 | someone moved the ref; re-read and retry (`retryable: true`) |
| `LeaseConflictError` | `lease_conflict` | 409 | another agent holds the path; `details = {path, holder, lease_id}` |
| `MergeConflictsError` | `merge_conflicts` | 409 | three-way merge needs a decision; `details = {count}` |
| `PolicyViolationError` | `policy_violation` | 403 | missing intent/approvals or a direct push to a protected branch |
| `NotFoundError` | `not_found`, `ref_not_found`, `unknown_revision`, `lease_not_found`, `object_not_found` (412) | 404/412 | check the name |
| `AgitError` | `already_exists`, `forbidden`, `invalid`, `lock_timeout`, ... | any | inspect `code` |

Transport failures are `AgitError`s with `status: 0`: `network` (server unreachable) and
`timeout` (per-request `timeoutMs`, default 30 s). Both are `retryable`. A caller's own
`AbortSignal` surfaces as the runtime's `AbortError`, unchanged.

```ts
import { isRetryable } from "@agit/sdk";

for (let attempt = 0; ; attempt++) {
  try {
    return await client.updateRefs(repo, updates);
  } catch (e) {
    if (!isRetryable(e) || attempt >= 3) throw e;
  }
}
```

## Objects

`encodeObjectStream` / `decodeObjectStream` implement the object stream format
(`kind:u8 | len:u32be | payload`, kinds 1 blob, 2 tree, 3 commit) used by
`uploadObjects`, `fetchObjects` and `POST /fetch`. Object ids are BLAKE3 digests computed
by the server; this SDK ships no hash implementation, so it cannot build commits locally
and point refs at them. Use the `agit` CLI (or the Rust/Python protocol clients) to commit
and push; use this SDK for everything around it.

## Environment

Constructor options win; otherwise these variables are read when available
(`process.env` on Node/Bun, `Deno.env` with `--allow-env`):

| variable | option | default |
| --- | --- | --- |
| `AGIT_SERVER_URL` | `baseUrl` | `http://127.0.0.1:7878/api/v1` |
| `AGIT_TOKEN` | `token` | none |
| `AGIT_AGENT` | `agent` | none (server records `anonymous`) |
| `AGIT_MODEL` | `model` | none |

## Development

```sh
pnpm install
pnpm build        # dist/index.js (ESM), dist/index.cjs (CJS), .d.ts
pnpm test         # vitest against an in-process fake server
AGIT_SERVER_URL=http://127.0.0.1:7878/api/v1 pnpm test   # also runs the live integration test
```

## License

MIT OR Apache-2.0, like the rest of agit.
