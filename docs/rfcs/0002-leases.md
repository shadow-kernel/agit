# RFC 0002: Leases

- Status: **Accepted**
- Phase: 0 (implemented in `crates/agit-core/src/lease.rs`)
- Supersedes: none

## Summary

A lease is an advisory, time-limited claim by a named holder on a set of path patterns, in exclusive or shared mode. Claims that overlap another holder's exclusive lease are refused. `agit commit` refuses to record changes under another holder's exclusive lease unless forced. Leases expire automatically.

## Motivation

When several agents work in one repository, the expensive failure is not the merge conflict; it is discovering the conflict after both agents have spent their budget producing incompatible work. git offers nothing between "talk to each other out of band" and "find out at merge time".

A lease moves the coordination point to the start of the work: an agent announces "I am editing `src/auth/**` for the next 30 minutes". Other agents see the claim immediately and choose different work, wait, or negotiate. Because agents crash, the claim must expire on its own. Because agents sometimes must override (a hotfix), the mechanism is advisory with an explicit, logged force.

## Design

### Data model

One JSON file per lease, `.agit/leases/<id>.json`:

| field | type | notes |
| --- | --- | --- |
| `id` | string | 12 hex characters from a random UUIDv4 |
| `holder` | string | agent id or user name |
| `paths` | string[] | literal paths or globs, at least one |
| `mode` | `exclusive` or `shared` | default exclusive |
| `created_ms` | i64 | |
| `expires_ms` | i64? | absent means no expiry |
| `note` | string? | human-readable purpose |
| `workspace` | string? | where the holder is working |

### Coverage

A literal pattern covers itself and every path beneath it. A glob (any of `* ? [ {`, compiled with `globset`) covers a path if it matches the path or any ancestor of the path, so `src/*` covers `src/a/b` for leasing purposes.

### Overlap

Two patterns overlap when they might cover a common path. For two literals that is equality or directory-prefix. When a glob is involved the test compares literal prefixes (text before the first glob character) and is deliberately conservative: an empty literal prefix overlaps everything, and a glob's literal prefix that is a plain string prefix of the other pattern overlaps (`src/foo*` vs `src/foobar/x`). False positives are accepted; false negatives are bugs.

### Operations

All under `.agit/locks/leases.lock` (advisory `flock`, 5 s timeout).

- **claim**: validate globs, purge expired leases, and for `exclusive` requests fail with `lease_conflict {path, holder, lease_id}` if any active exclusive lease of a *different* holder overlaps any requested path. Same-holder overlaps are allowed. Shared leases never conflict in either direction.
- **renew**: set `expires_ms = now + ttl`.
- **release**: delete the file. **release_all(holder)** releases every lease of a holder, for agent shutdown.
- **list**: active leases only, sorted by `created_ms`.
- **purge_expired**: lazy expiry; also runs inside every claim.

Each operation appends a `lease.*` event to the event log.

### Enforcement

`agit commit` computes the set of paths that differ from the parent tree and asks the lease store for a conflict with holder = `--agent` id, or the configured user when no agent is given. A conflict aborts the commit with `lease_conflict`. `--force` bypasses the check and the resulting event records `forced: true`. `merge` performs the same check against the paths changed by the incoming side.

Leases do not intercept filesystem writes. They gate the act of recording a change, which is the point at which the change becomes visible to others.

### Expiry

`expires_ms <= now` means expired. Listings filter expired leases; files are removed lazily. No daemon is required. Clock skew between agents on one machine is not an issue because there is one clock; on the server (📋) the server clock is authoritative.

### JSON contract

```sh
agit lease claim src/auth 'docs/auth/**' --holder coder-2 --ttl 30m --note "refactor login" --json
```

```json
{"ok":true,"result":{"id":"a1b2c3d4e5f6","holder":"coder-2","paths":["src/auth","docs/auth/**"],"mode":"exclusive","created_ms":1760480000123,"expires_ms":1760481800123,"note":"refactor login","workspace":"coder-2"}}
```

`--holder` defaults to `$AGIT_AGENT`; `--ttl` defaults to `1h` and accepts `s`, `m`, `h`, `d`; `--shared` requests a shared lease. `agit lease release <id>` releases one lease, `agit lease release --all --holder <h>` releases every lease of a holder (agent shutdown). `agit lease check <paths…> --holder <h>` answers whether a holder may modify the paths without claiming anything.

Conflict (exit code 3):

```json
{"ok":false,"error":{"code":"lease_conflict","message":"lease conflict: src/auth/login.rs is held by coder-1 (lease 0f9e8d7c6b5a)","retryable":false}}
```

`lease_conflict` is not retryable: repeating the same command will fail until the other lease is released or expires. Agents should wait on the event stream for `lease.release` of that lease id, or until its `expires_ms` has passed (expiry is lazy, no event is emitted for it today), or pick other work.

## Alternatives

- **Mandatory locks (refuse file writes).** Rejected: agit does not own the filesystem, and mandatory locks turn a crashed agent into a blocked team. Advisory with TTL plus a logged force is the sweet spot.
- **Lease per file instead of per pattern.** Rejected: agents reason about modules and directories; a pattern claim is one operation instead of hundreds.
- **Exact glob intersection for overlap.** Rejected for now: general glob intersection is complex and the conservative prefix test errs on the safe side. Can be tightened later without a format change.
- **Leases stored as refs or objects.** Rejected: leases are ephemeral coordination state, not history. They should not be cloned or merged. On the server they will be replicated through the commit log but not stored as objects.

## Open questions

- Whether the server should auto-release leases when the holder's token expires or its workspace is removed. Likely yes, with a `lease.expire` event.
- A `lease wait` command that blocks until a pattern is free, with a timeout, as a convenience over tailing events.
- Priority or preemption (a human overriding an agent) as a policy rather than `--force`.
- Whether `shared` leases should produce a warning event when an exclusive claim covers them, so a reader learns that its inputs are about to change.
