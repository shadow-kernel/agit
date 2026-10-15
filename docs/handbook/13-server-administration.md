# 13. Server administration

`agit-server` is one static binary, an axum HTTP server that keeps every repository as plain files under one data directory. No database, no cache, no background worker. This chapter covers running it, what is on disk, backups, tokens, policies, keys and webhooks, monitoring and upgrades. [docs/DEPLOYMENT.md](../DEPLOYMENT.md) has the systemd unit, Caddy/nginx configurations and sizing guidance in full; [docs/SECURITY.md](../SECURITY.md) has the threat model and hardening checklist.

Status: single node, file-backed, one root token plus scoped tokens. Replication, sharding and organisations are 📋.

## Running

```sh
agit-server --data-dir /var/lib/agit --listen 127.0.0.1:7878
# equivalent
AGIT_DATA_DIR=/var/lib/agit AGIT_LISTEN=127.0.0.1:7878 agit-server
```

| Variable | Flag | Default | Purpose |
| --- | --- | --- | --- |
| `AGIT_DATA_DIR` | `--data-dir` | `./agit-data` | directory holding all repositories; the only state |
| `AGIT_LISTEN` | `--listen` | `127.0.0.1:7878` | socket address (`0.0.0.0:7878` in containers) |
| `AGIT_SERVER_TOKEN` | `--token` | unset (open mode) | root bearer token; an empty value is a (bad) token, not "off" |
| `AGIT_WEB_DIR` | `--web-dir` | unset | built web UI, served at `/` with SPA fallback, without token check |
| `RUST_LOG` | | `info,tower_http=info` | log filter; `tower_http=debug` logs each request |
| `AGIT_FSYNC` | | `refs` | durability: `refs`, `all`, `none` |

Flags win over the environment. On start the server logs one line (`agit-server listening`, with `addr`, `data_dir`, `auth=true|false`, `web`). Logs go to stdout/stderr (journald under systemd, `docker logs` in containers).

Check it:

```sh
$ curl -s http://127.0.0.1:7878/api/v1/health
{"status":"ok","version":"0.1.0"}
```

`/api/v1/health` answers without a token (DEPLOYMENT.md says otherwise; the binary serves it openly), so proxies and container health checks can probe it directly. Everything else under `/api/v1` requires the token when one is set.

### Docker and Compose

```sh
docker run -d --name agit -p 127.0.0.1:7878:7878 -v agit-data:/data \
  -e AGIT_SERVER_TOKEN="$(openssl rand -hex 32)" ghcr.io/shadow-kernel/agit:0.1.0
```

Bind mounts must be writable by uid 10001. `docker-compose.yml` binds `127.0.0.1:7878`, passes `AGIT_SERVER_TOKEN` through only when set on the host (do not write `AGIT_SERVER_TOKEN=` with an empty value), and defines a health check that sends the token. `docker compose up -d --build` builds from the checkout.

### systemd

`deploy/agit-server.service` (hardened: `ProtectSystem=strict`, `NoNewPrivileges`, empty capability set, syscall filter, `StateDirectory=agit`, `LimitNOFILE=65536`) and `deploy/agit-server.env`. Install the binaries to `/usr/local/bin`, the env file to `/etc/agit/agit-server.env` (mode 0640, root:agit), set the token there, `systemctl enable --now agit-server`. The unit listens on `127.0.0.1:7878` and expects a reverse proxy.

### Reverse proxy

The server speaks plain HTTP; terminate TLS in Caddy (`deploy/Caddyfile`) or nginx (`deploy/nginx.conf`). Two settings matter: disable response buffering and raise read timeouts for `/api/v1/repos/*/events/stream` (long-lived SSE), and raise the request body limit (`client_max_body_size 0` in nginx) because `agit push` uploads object streams in one request. Forward `Authorization` unchanged. The server sets `Access-Control-Allow-Origin: *`; since the credential is a header, not a cookie, this does not expose authenticated sessions to other sites.

## Data layout

```
<data-dir>/
  audit.jsonl                 denied requests and every mutation: {time_ms, principal, agent, method, path, status}
  auth/                       scoped tokens (hashes) and metadata
  repos/<repo>/
    meta.json                 {name, default_branch, created_ms}
    objects/pack/             append-only segments seg-<pid>-<rand>.agp with .idx (AGX1) and .lock files
    refs/heads/ refs/tags/    one small file per ref, written atomically
    logs/refs/heads/…         reflogs per ref
    events.jsonl              append-only event log with sequence numbers
    leases/                   active server leases
    pulls/<n>.json, counter   pull requests with comments, reviews, checks
    knowledge/notes/<id>.json knowledge notes
    knowledge/rules.json      tag rules
    policy.json               when set
    keys.json                 trusted signing keys per agent, when set
    webhooks.json             subscriptions including their secrets (plaintext)
    locks/                    advisory flock files; empty, safe to delete when stopped
```

A repository created through the API has a bare layout (no working tree). Objects are immutable and verified on read; refs are atomic temp+rename writes; `events.jsonl` and `audit.jsonl` are append-only.

## Backups

The data directory is the backup. Properties that make it easy: objects are content-addressed and idempotent to copy; refs are written atomically, so copying `objects/` before `refs/` yields a consistent snapshot even while the server runs; `events.jsonl` copied mid-write is simply a shorter log.

In order of consistency:

1. **Filesystem snapshot** (ZFS, btrfs, LVM, cloud disk) of the volume.
2. **Stop, copy, start**: `systemctl stop agit-server`, `tar`/`rsync` the directory, start.
3. **Live `rsync`** twice (`rsync -a --delete data/ backup/`), objects first by virtue of path order; the second pass closes the gap.

For Docker: `docker run --rm -v agit-data:/data:ro -v "$PWD":/out alpine tar -C / -czf /out/agit-data.tgz data`. Ship copies off-host and **encrypt them**: `webhooks.json` holds webhook secrets, `auth/` holds token hashes, and notes may contain anything agents wrote. Exclude nothing except `locks/`.

**Restore**: stop the server, put the tree back at `AGIT_DATA_DIR`, fix ownership (`chown -R agit:agit`, or uid 10001 in Docker), start, then `agit clone` a repository and run `agit fsck` in the clone. Do this on a schedule, not only in emergencies.

## Authentication

### The root token

`AGIT_SERVER_TOKEN` grants every permission and bootstraps everything else. Generate it with `openssl rand -hex 32`, keep it out of shell history (env file, Docker secret), rotate by restarting with a new value (no overlap window; update clients first or accept a few seconds of `403`). Without a token the server is **open**: every request acts as an anonymous root. Acceptable on `127.0.0.1` for development, nowhere else.

Clients: `AGIT_TOKEN` for the CLI, the `token` option in the SDKs, the settings popover in the web UI.

### Scoped tokens

Create per-agent or per-job tokens with the root token (or any admin-capable token, which can only grant scopes it holds; `*` scopes require root):

```sh
$ curl -s -X POST -H "Authorization: Bearer $ROOT" -H 'content-type: application/json' $U/auth/tokens \
    -d '{"name":"coder-1 laptop","agent":"coder-1","scopes":[{"repo":"demo","access":"write"}],"ttl_secs":3600}'
{"token":"agit_da83a014…","id":"tok_803401c4e6","name":"coder-1 laptop","agent":"coder-1","scopes":[{"repo":"demo","access":"write"}],"created_ms":…,"expires_ms":…,"created_by":"root"}
```

- `scopes`: `{repo: "<name>" | "*", access: "read" | "write" | "admin"}`. `GET` needs read; other methods write; repository create/delete, `policy`, `keys`, `webhooks`, `knowledge/rules` and `/auth/*` need admin.
- `agent`: binds the token to an identity. Requests made with it act as that agent regardless of `X-Agit-Agent`, so events, leases, PR authorship and policy checks cannot be spoofed by header. (Request bodies that carry a holder, such as `POST /leases`, are not rewritten yet.)
- `ttl_secs`: optional expiry. The plaintext token is returned once; `GET /auth/tokens` shows metadata only.
- `DELETE /auth/tokens/{id}` revokes immediately (`403` on the next request).
- `GET /auth/whoami` lets a client confirm what it is: `{"principal":"tok_…","agent":"coder-1","root":false,"scopes":[…]}`.

Recommended: one token per agent *and* machine, write scope on the repositories it works in, bound to its id, with a TTL matching the job; admin tokens only for operators and the deployment pipeline; root only to mint tokens.

### Audit log

`<data-dir>/audit.jsonl` receives every non-GET request and every denied request:

```json
{"time_ms":1792093213997,"principal":"root","agent":null,"method":"POST","path":"/auth/tokens","status":201}
{"time_ms":1792093214018,"principal":"tok_803401c4e6","agent":"coder-1","method":"PUT","path":"/repos/demo/policy","status":403}
```

Ship it with the rest of your logs; it is the record of who changed what.

## Policies

Per repository, `GET/PUT /repos/{repo}/policy` (admin), also editable in the web UI under Settings. Fields, semantics and templates: [5](05-concepts.md#policies), [12](12-multi-agent-workflows.md#policy-templates). Operational notes:

- Setting a policy emits `policy.update` with the full document, so the event log and webhooks record policy changes.
- `require_signed` needs a populated key registry first, or every merge will fail.
- `allowed_merge_agents`/`allowed_push_agents` compare against the acting agent id; with scoped tokens bound to agents this is a real control, with header-only identity it is a guard rail.
- Policies are evaluated on `refs/update` (pushes) and `pulls/{n}/merge` only; local operations are unaffected.

## Signing keys

The registry maps agent ids to Ed25519 public keys (`keys.json`):

```sh
curl -s -H "$H" $U/repos/demo/keys
curl -s -X POST -H "$H" -H 'content-type: application/json' $U/repos/demo/keys -d '{"agent":"coder-1","public_key":"<64 hex>"}'
curl -s -X DELETE -H "$H" $U/repos/demo/keys/<key_id>
```

Agents register their own key with `agit key generate --register` or `agit key register`, which needs admin scope on the repository, so in a locked-down deployment an operator registers keys as part of onboarding (`agit key show --json` prints the public half). A key registered for one agent cannot be registered for another (409). Removing a key makes commits signed with it `trusted: false` from then on; already merged history is unaffected.

## Webhooks

Per repository, admin scope. `POST /webhooks {url, kinds?, secret?}` (http or https URLs only), `GET`, `DELETE /webhooks/{id}`, `POST /webhooks/{id}/test` queues a `ping`. Deliveries run on a background thread with three attempts (200 ms, 800 ms, 3.2 s backoff), carry `X-Agit-Event`, `X-Agit-Repo`, `X-Agit-Delivery`, `X-Agit-Webhook` and, with a secret, `X-Agit-Signature: blake3=<hex>` (keyed BLAKE3 over the body with `derive_key("agit webhook v1", secret)`). Kind filters: exact kinds, `pull.*`, `pull.` or `*`.

Operational notes: the server performs outbound HTTP to any URL a token holder registers (an SSRF surface); restrict egress with a firewall or network policy if the server can reach internal services. Secrets are stored in plaintext in `webhooks.json`. Failed deliveries are logged at `warn` and dropped after the third attempt; there is no dead-letter queue, so receivers should also poll `GET /events?since=` to catch up.

## Monitoring

- **Liveness**: `GET /api/v1/health` (no token required).
- **Activity**: `GET /repos/{repo}/events?since=<seq>` or the SSE stream for commits, ref updates, PR actions, leases, notes, policy and webhook changes; `GET /repos/{repo}/agents` for per-agent activity. A simple watcher that alerts on `ref.update` with `force`, `policy.update`, `key.*`, `webhook.*` and repository deletions covers most "something unexpected happened" cases.
- **Audit**: tail `audit.jsonl` for `status: 403` bursts (misconfigured clients or probing) and for admin mutations.
- **Capacity**: disk under `repos/*/objects` is the number to graph; pushes and merges are fsync-bound (`AGIT_FSYNC=refs`), so IOPS and latency matter more than throughput; one SSE connection per watching agent; `RUST_LOG=info,tower_http=debug` gives per-request latency when needed.
- Sizing starting points (not measurements): 1 vCPU / 512 MB for personal use, 2 vCPU / 1-2 GB for a team with tens of agents, 4+ vCPU / 4 GB / NVMe for hundreds of concurrent agents. Scale vertically; one process and one data directory per server, one server per trust domain.

## Upgrades

1. Read the [CHANGELOG](../../CHANGELOG.md). Before 1.0 a minor version may change on-disk formats; such changes are listed under "Changed" with the affected format and whether old data is read transparently (for example, `AGI1` indexes and loose `AGO1` objects are still read by 0.1.0).
2. Back up.
3. Replace the binary and restart (`systemctl restart agit-server`; Docker: pull and recreate, the volume carries the data). Unpack the new web tarball if you serve the UI.
4. Check `GET /api/v1/health` reports the new version and `agit pull` works from a client.

Clients and server should run the same minor version while the API is `v1` and pre-1.0; the server rejects unknown object formats rather than guessing. Downgrading works as long as the data format did not change; otherwise restore the backup.

## Hardening checklist (short form)

- Root token of at least 32 random bytes; never an open server on a reachable interface.
- Bind to `127.0.0.1`, TLS in a reverse proxy, HSTS once TLS works.
- Dedicated unprivileged user, data directory mode `0750`, the shipped systemd hardening or equivalent container flags.
- Body size and connection rate limits at the proxy; restrict outbound traffic to known webhook receivers.
- Encrypt the volume and the backups.
- Scoped tokens bound to agents for every agent; rotate on decommission or leak; keep tokens out of prompts and transcripts.
- Policies on protected branches, with the understanding that they catch mistakes, not impostors.
- One server per trust domain.

The full list, the threat model and the disclosure process are in [docs/SECURITY.md](../SECURITY.md).

## Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| `403 forbidden` on every call | token set on the server but missing or different on the client; check for an empty `AGIT_SERVER_TOKEN=` |
| `403 … token lacks Admin access` | a scoped token hitting an admin endpoint (policy, keys, webhooks, rules, repo create/delete); use an admin or root token |
| container exits with a permission error on `/data` | bind mount not writable by uid 10001 |
| SSE arrives in bursts or drops behind a proxy | response buffering or read timeout on the proxy |
| `413` or failed pushes behind nginx | `client_max_body_size` too small |
| web UI shows "Cannot reach …" | API base URL points elsewhere; set it in the settings popover or rebuild with `VITE_AGIT_API` |
| `ref_conflict` from clients | expected under concurrent pushes; clients retry |
| `GET /refs?prefix=refs/heads/main` returns an `io` error | the prefix names a file; use a directory prefix (`refs/heads/`) |
| slow pushes on network storage | fsync latency; use local SSD, do not set `AGIT_FSYNC=none` on the only copy |
