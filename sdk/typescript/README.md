# agit TypeScript SDK (planned)

Status: 📋 planned (Roadmap Phase 4). This directory holds only this README.

A typed TypeScript client for agents, orchestrators, and the web UI. It will
expose the same operations as the Python SDK against both the local
`agit --json` CLI and the server API:

```ts
import { Repo } from "@agit/sdk";

const repo = await Repo.open(".");
const lease = await repo.lease.claim(["src/auth/**"], { holder: "coder-2", ttl: "30m" });
await repo.add(["src/auth/login.ts"]);
await repo.commit({
  message: "Add login endpoint",
  intent: "Allow users to sign in with email",
  task: "PROJ-123",
  agent: { id: "coder-2", model: "<model>", runId: "run-42" },
});
for await (const event of repo.events.follow({ since: lease.seq })) {
  // react to other agents
}
```

Errors carry the stable `code` and `retryable` flag from the JSON error
contract. Types will be generated from the versioned JSON schemas in `spec/`.
The shape above is illustrative.
