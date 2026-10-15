# RFC 0005: Pull requests and reviews

- Status: **Draft**
- Phase: 3
- Depends on: RFC 0001, RFC 0002, RFC 0004 (server)

## Summary

A pull request is a structured server-side object: title, intent, task, agent, source and target refs, reviewers, checks, and an append-only decision log. Review comments are structured: path, range, kind (`question`, `suggestion`, `blocker`), and resolution. Agents open, review and merge PRs through the same API humans use. Policies decide which principals may merge what, and every decision is recorded.

## Motivation

On today's platforms a PR is a title, a markdown body and a thread of free-text comments. An agent that reviews has to infer whether a comment blocks the merge; an agent that authors has to parse prose to find out what to fix; an orchestrator cannot tell which checks matter. Making the PR a typed object with explicit semantics removes the guessing. Recording the decision log makes agent merges auditable.

## Design

### PR object

```json
{
  "version": 1,
  "id": 42,
  "repo": "org/project",
  "title": "Add login endpoint",
  "intent": "Allow users to sign in with email",
  "task": "PROJ-123",
  "agent": {"id": "coder-2", "model": "my-model-v3", "run_id": "run-0042"},
  "author": {"name": "Daniel", "email": "daniel@example.com"},
  "source": {"ref": "refs/heads/feature/login", "head": "<id>"},
  "target": {"ref": "refs/heads/main", "base": "<id>"},
  "state": "open",
  "reviewers": [{"principal": "reviewer-1", "required": true, "state": "pending"}],
  "checks": [{"name": "tests", "state": "passed", "url": "…", "time_ms": 0}],
  "labels": ["area:auth"],
  "created_ms": 0,
  "updated_ms": 0
}
```

`state` is `open`, `merged`, `closed`. `source.head` is the id at the time of the last update; the server records every head change in the decision log so a review can be pinned to the revision it saw.

### Review comments

```json
{
  "id": "c-17",
  "pr": 42,
  "revision": "<source head id reviewed>",
  "path": "src/auth/login.rs",
  "range": {"start_line": 40, "end_line": 48},
  "kind": "blocker",
  "body": "Token is logged in plain text.",
  "suggestion": {"replace_with": "…optional patch text…"},
  "author": {"principal": "reviewer-1", "agent": {"id": "reviewer-1"}},
  "resolution": {"state": "open"}
}
```

`kind` is `question`, `suggestion` or `blocker`. `resolution.state` is `open`, `resolved`, `wont_fix`, with `by`, `time_ms` and an optional `commit` that addressed it. A PR with any open `blocker` on its current revision cannot merge. `suggestion` comments may carry a patch that the author (human or agent) can apply with one API call, which produces a commit whose `meta.agit.suggestion` records the comment id.

### Reviews

A review is a verdict on a revision: `approve`, `request_changes`, `comment`, with the set of comment ids it contains. Approvals are tied to the reviewed revision; a new head invalidates them unless policy says otherwise (for example, trivial rebases).

### Decision log

Append-only per PR: opened, head updated, review submitted, check reported, policy evaluated (with the rule and outcome), merged by whom with which strategy, closed. It is the audit trail that makes agent-driven merging acceptable, and it is exposed through the event stream as `pr.*` events.

### Merge

`POST /repos/{repo}/prs/{id}/merge` with strategy `merge`, `squash` or `rebase`. The server evaluates policy, performs the merge with the core three-way algorithm, writes the result as a log entry with a CAS on the target ref, and records the decision. Squash keeps provenance (agents, intents, checkpoints) under `meta.agit.squashed`. Cross-repository PRs ("stacked across repos", RFC 0007) are a set of PRs that merge atomically or not at all.

### Policies

Declared per repository and branch pattern, evaluated at merge time and at ref update time:

- who may open, review, approve and merge (principals, agent ids, token scopes);
- required reviewers and required checks;
- required commit fields (`intent`, `task`), maximum files or bytes per commit, maximum commits per PR;
- whether agents may approve agent-authored PRs;
- whether approvals survive a head change.

A violation is `policy_violation` with the rule name; never retryable.

### API

REST over HTTP/2 and the same operations over gRPC. All responses carry `version`. The web UI uses exactly these endpoints.

## Alternatives

- **Markdown body with conventions (`BLOCKER:` prefixes).** Rejected: conventions drift and cannot be enforced.
- **Comments as commits in the repository.** Rejected: review state is not repository content; it would pollute history and clone size. The decision log is however exportable as JSON for archival.
- **Letting agents merge without policy.** Rejected: policy is the only control that scales to many agents.

## Open questions

- Whether a PR should be able to target a workspace or a checkpoint ref for intra-agent "self review" flows.
- Threading model for comments (flat with `reply_to` versus nested).
- How to represent file-level and PR-level comments without a path or range (likely `path: null`).
- Interaction with leases: whether opening a PR should release the author's leases on the changed paths, or convert them to shared.
- Whether checks should be first-class objects with their own API (likely yes, in a follow-up RFC).
