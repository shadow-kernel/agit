# agit Python SDK (planned)

Status: 📋 planned (Roadmap Phase 4). This directory holds only this README.

A thin, typed Python client for agents and orchestrators. It will wrap the
`agit --json` CLI locally and the server API remotely behind one interface:

```python
from agit import Repo

repo = Repo.open(".")
lease = repo.lease.claim(["src/auth/**"], holder="coder-2", ttl="30m")
repo.add(["src/auth/login.py"])
repo.commit(message="Add login endpoint",
            intent="Allow users to sign in with email",
            task="PROJ-123",
            agent={"id": "coder-2", "model": "<model>", "run_id": "run-42"})
for event in repo.events.follow(since=lease.seq):
    ...
```

Errors are raised as typed exceptions carrying the stable `code` and
`retryable` flag from the JSON error contract. The SDK will also ship the
MCP server glue (`agit mcp`).

The shape above is illustrative; the actual API will follow the versioned JSON
schemas in `spec/`.
