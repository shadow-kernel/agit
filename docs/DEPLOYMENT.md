# Deploying agit-server

`agit-server` is a single static binary: an axum HTTP server that keeps every
repository as plain files under one data directory. There is no database, no
cache and no background worker to run beside it. This guide covers the ways to
run it, how to put TLS and authentication in front of it, and how to back it up
and upgrade it. The security model is in [SECURITY.md](SECURITY.md); the HTTP
contract is in [API.md](API.md).

Status: the server is single-node and file-backed (Phase 2/3 of the
[roadmap](ROADMAP.md)). Replication, sharding and per-agent credentials are
designed but not built. Treat the deployment advice here as the current state,
not a promise about the distributed version.

## Contents

- [Pick a method](#pick-a-method)
- [Single binary](#single-binary)
- [Docker](#docker)
- [Docker Compose](#docker-compose)
- [systemd](#systemd)
- [Reverse proxy with TLS](#reverse-proxy-with-tls)
- [Authentication token](#authentication-token)
- [Web UI](#web-ui)
- [Environment variables](#environment-variables)
- [Data directory and backups](#data-directory-and-backups)
- [Upgrades](#upgrades)
- [Resource sizing](#resource-sizing)
- [Logging and monitoring](#logging-and-monitoring)
- [Troubleshooting](#troubleshooting)

## Pick a method

| You want | Use |
| --- | --- |
| A server for yourself or a demo | [Single binary](#single-binary), `agit-server --data-dir ./agit-data` |
| A server on a VM for a team | [systemd](#systemd) behind [Caddy or nginx](#reverse-proxy-with-tls) |
| Containers | [Docker](#docker) or [Compose](#docker-compose), with a proxy for TLS |
| macOS laptop service | `brew services start agit` from the Homebrew formula (`homebrew/agit.rb`) |

Whichever you pick, three things matter: the data directory is the only state,
the bearer token is the only authentication, and TLS is the proxy's job.

## Single binary

Install the release binaries (both `agit` and `agit-server`):

```sh
curl -fsSL https://raw.githubusercontent.com/shadow-kernel/agit/main/scripts/install.sh | sh
# options: --version v0.1.0, --prefix /usr/local, --bin-dir DIR, --dry-run
```

The installer detects macOS (arm64, x86_64) and Linux (x86_64, aarch64),
downloads `agit-<version>-<target>.tar.gz` from the GitHub release, verifies
the `.sha256` file, and installs into `~/.local/bin` (or `/usr/local/bin` when
run as root or with `--prefix /usr/local`). Windows users download the
`x86_64-pc-windows-msvc.zip` asset. To build from source instead:
`cargo build --release` (Rust 1.89 or newer) produces
`target/release/{agit,agit-server}`.

Run it:

```sh
agit-server --data-dir /var/lib/agit --listen 127.0.0.1:7878
# equivalent: AGIT_DATA_DIR=/var/lib/agit AGIT_LISTEN=127.0.0.1:7878 agit-server
curl -s http://127.0.0.1:7878/api/v1/health      # {"status":"ok","version":"0.1.0"}
```

The data directory is created if missing. Pass `--token` (or
`AGIT_SERVER_TOKEN`) to require authentication and `--web-dir` to serve the web
UI; both are covered below. `agit-server --help` lists every flag.

Point clients at `http://<host>:7878/api/v1/repos/<name>`:

```sh
agit remote add origin http://127.0.0.1:7878/api/v1/repos/demo
agit push --create
```

## Docker

Images are published to `ghcr.io/shadow-kernel/agit` for `linux/amd64` and
`linux/arm64`, tagged `<version>`, `<major.minor>` and `latest`. The image
contains `agit-server`, the `agit` CLI, `git` (for `agit git import|export`)
and the built web UI, runs as the unprivileged user `agit` (uid/gid 10001), and
has a health check against `/api/v1/health`.

```sh
docker run -d --name agit \
  -p 127.0.0.1:7878:7878 \
  -v agit-data:/data \
  -e AGIT_SERVER_TOKEN="$(openssl rand -hex 32)" \
  ghcr.io/shadow-kernel/agit:0.1.0
docker logs -f agit
```

Image defaults: `AGIT_DATA_DIR=/data`, `AGIT_LISTEN=0.0.0.0:7878`,
`AGIT_WEB_DIR=/opt/agit/web`, `RUST_LOG=info,tower_http=info`. Container
arguments are passed to `agit-server`, so `docker run ... ghcr.io/shadow-kernel/agit
--help` works. `tini` is PID 1, so `docker stop` delivers SIGTERM promptly.

Bind mounts instead of a named volume need to be writable by uid 10001:

```sh
mkdir -p /srv/agit && sudo chown 10001:10001 /srv/agit
docker run -d -v /srv/agit:/data ...
```

Use the CLI from the image (for example, to import a git repository on a host
without agit installed):

```sh
docker run --rm -v "$PWD":/work -w /work --entrypoint agit ghcr.io/shadow-kernel/agit:0.1.0 --version
docker exec agit agit --version
```

Build locally from a checkout:

```sh
docker build -t agit:dev .
# API base URL baked into the UI (default /api/v1, relative to the page origin):
docker build --build-arg AGIT_WEB_API=https://agit.example.com/api/v1 -t agit:dev .
```

The build compiles the Rust workspace in release mode (several minutes on the
first run; cargo and pnpm caches are BuildKit cache mounts, so rebuilds are
fast). The release workflow builds with `--build-arg BINARIES=prebuilt` to reuse
the already-built Linux binaries; that mode expects
`dist/linux-{amd64,arm64}/{agit,agit-server}` in the build context and is not
needed locally.

## Docker Compose

[`docker-compose.yml`](../docker-compose.yml) in the repository root runs the
image with a named volume and passes the token through from the environment:

```sh
export AGIT_SERVER_TOKEN="$(openssl rand -hex 32)"     # or put it in ./.env
docker compose up -d
docker compose logs -f
docker compose up -d --build                           # build from this checkout
```

The file binds the port to `127.0.0.1:7878` on the host, expecting a reverse
proxy for TLS; change it to `"7878:7878"` to expose the server on a private
network directly. The commented `volumes:` entries show how to use a host
directory instead of the named volume and how to mount an existing `repos`
directory from another agit server.

`AGIT_SERVER_TOKEN` is listed without a value so it is only set in the
container when it is set on the host. Do not write `AGIT_SERVER_TOKEN=` with an
empty value: the server treats an empty string as a token (see
[Authentication token](#authentication-token)).

## systemd

Files are in [`deploy/`](../deploy): `agit-server.service` (hardened unit) and
`agit-server.env` (settings). The unit runs as the `agit` user, keeps data in
`/var/lib/agit` (`StateDirectory=agit`), reads settings from
`/etc/agit/agit-server.env`, restarts on failure, and locks the process down
with `ProtectSystem=strict`, `NoNewPrivileges`, an empty capability set and a
system-call filter. It needs nothing outside its state directory.

```sh
sudo useradd --system --home-dir /var/lib/agit --shell /usr/sbin/nologin agit
sudo install -m 0755 agit agit-server /usr/local/bin/
sudo mkdir -p /etc/agit
sudo install -m 0640 -o root -g agit deploy/agit-server.env /etc/agit/agit-server.env
sudo install -m 0644 deploy/agit-server.service /etc/systemd/system/agit-server.service
sudoedit /etc/agit/agit-server.env            # set AGIT_SERVER_TOKEN, AGIT_WEB_DIR
sudo systemctl daemon-reload
sudo systemctl enable --now agit-server
systemctl status agit-server
journalctl -u agit-server -f
```

The env file is `KEY=value` lines without shell expansion, so generate the
token separately (`openssl rand -hex 32`) and paste it. To serve the web UI,
unpack `agit-web-<version>.tar.gz` from the release into `/opt/agit/web` and
set `AGIT_WEB_DIR=/opt/agit/web`. The unit listens on `127.0.0.1:7878`; to bind
port 80 or 443 directly instead of using a proxy, uncomment the
`CAP_NET_BIND_SERVICE` lines in the unit.

## Reverse proxy with TLS

`agit-server` speaks plain HTTP. Terminate TLS in a reverse proxy on the same
host and keep the server bound to `127.0.0.1`. Two things need attention in any
proxy configuration:

1. **Server-sent events.** `GET /api/v1/repos/{repo}/events/stream` is a
   long-lived response. Disable response buffering and raise or remove read
   timeouts for that path, or clients will see events arrive late or the
   stream cut off.
2. **Large request bodies.** `agit push` uploads object streams in one request.
   Raise the body size limit (or remove it) and avoid spooling request bodies
   to disk.

Also forward `Authorization` unchanged (both examples do) and set HSTS once
TLS works.

**Caddy** ([`deploy/Caddyfile`](../deploy/Caddyfile)) obtains and renews
certificates automatically:

```sh
sudo install -m 0644 deploy/Caddyfile /etc/caddy/Caddyfile   # change agit.example.com
sudo systemctl reload caddy
```

**nginx** ([`deploy/nginx.conf`](../deploy/nginx.conf)) with certificates from
certbot:

```sh
sudo install -m 0644 deploy/nginx.conf /etc/nginx/sites-available/agit
sudo ln -s /etc/nginx/sites-available/agit /etc/nginx/sites-enabled/agit
sudo certbot --nginx -d agit.example.com
sudo nginx -t && sudo systemctl reload nginx
```

Health checks from the proxy or a load balancer go to `/api/v1/health` and
must carry the bearer token when one is set (the health route sits behind the
same middleware as everything else). A proxy health check that cannot send a
header can instead probe the TCP port.

The server sets permissive CORS headers (`Access-Control-Allow-Origin: *`) so
the web UI can run on a different origin. Because authentication is a header,
not a cookie, this does not grant other websites access to an authenticated
session; see [SECURITY.md](SECURITY.md).

## Authentication token

Set `AGIT_SERVER_TOKEN` (or `--token`) and every request, including
`/api/v1/health`, must carry `Authorization: Bearer <token>`. Requests without
it get `403`. Without a token the server is open to anyone who can reach the
port, which is acceptable only on a trusted private network or `127.0.0.1`.

```sh
openssl rand -hex 32                                  # generate
AGIT_SERVER_TOKEN=... agit-server ...                 # server
export AGIT_TOKEN=...                                 # agit CLI (clone, push, pull, pr, know)
curl -H "Authorization: Bearer $AGIT_TOKEN" https://agit.example.com/api/v1/repos
```

SDKs take the token in their client constructor; the web UI stores it in the
browser's localStorage after you enter it in the settings popover (top-right).
Clients identify the acting agent with `X-Agit-Agent` (set by `AGIT_AGENT` in
the CLI); that header is informational and is not checked against the token.

Rules of thumb:

- **One token, full access.** Everyone holding the token can read and write
  every repository and change policies and webhooks. Per-agent credentials and
  permissions are planned (RFC 0004); until then, run one server per trust
  domain.
- **Unset, not empty.** `AGIT_SERVER_TOKEN=""` configures an empty token
  rather than disabling authentication. Leave the variable unset to run open.
- **Rotation** is a restart with a new value; there is no overlap period, so
  update clients first or accept a brief window of `403`s.
- Keep the token out of shell history and process listings where possible:
  the systemd env file and Docker secrets or `.env` files are better than
  `--token` on the command line.

## Web UI

The release ships the built UI as `agit-web-<version>.tar.gz`; the Docker
image includes it at `/opt/agit/web`. Serving it from `agit-server --web-dir`
is the simplest arrangement: the UI then calls the API at the relative path
`/api/v1` on the same origin, which the release bundle is built for
(`VITE_AGIT_API=/api/v1`). If you host the static files elsewhere (object
storage, another web server), either build with `VITE_AGIT_API` set to the
server's absolute API URL or let users enter it in the settings popover.

## Environment variables

Every flag of `agit-server` has an environment variable; flags win over the
environment.

| Variable | Flag | Default | Purpose |
| --- | --- | --- | --- |
| `AGIT_DATA_DIR` | `--data-dir` | `./agit-data` | Directory holding all repositories. The only state the server has. |
| `AGIT_LISTEN` | `--listen` | `127.0.0.1:7878` | Socket address to bind (`0.0.0.0:7878` in containers, `[::]:7878` for IPv6). |
| `AGIT_SERVER_TOKEN` | `--token` | unset (no auth) | Bearer token required on every request. An empty value is a (bad) token, not "off". |
| `AGIT_WEB_DIR` | `--web-dir` | unset | Directory with the built web UI (`index.html` and assets), served at `/` with SPA fallback. |
| `RUST_LOG` | – | `info,tower_http=info` | Log filter (`tracing-subscriber` EnvFilter). `debug` or `tower_http=debug` logs each request; `warn` for quiet servers. |
| `AGIT_FSYNC` | – | `refs` | Durability: `refs` fsyncs ref updates and logs (git-like), `all` also fsyncs every object write, `none` never fsyncs (throwaway servers only). |

Client-side variables for completeness: `AGIT_TOKEN` (bearer token for `agit`),
`AGIT_AGENT`, `AGIT_MODEL`, `AGIT_RUN_ID` (commit and request metadata),
`AGIT_SERVER_URL` (SDK tests and examples). The SDKs read the same names.

## Data directory and backups

Everything lives under `AGIT_DATA_DIR`:

```
<data-dir>/
  repos/
    <repo>/
      meta.json        name, description, default branch, created time
      objects/         content-addressed blobs/trees/commits (immutable, zstd)
      refs/heads/ refs/tags/   one small file per ref (atomic temp+rename)
      logs/            per-ref update logs (reflog equivalent)
      events.jsonl     append-only event log with sequence numbers
      leases/          active leases
      pulls/           pull requests, comments, reviews, counter
      knowledge/       notes/ and rules.json
      policy.json      branch protection and merge rules (when set)
      webhooks.json    webhook subscriptions, including their secrets
      keys.json        trusted signing keys per agent (when set)
      locks/           advisory flock files; empty content, safe to delete when stopped
```

Snapshot the whole `repos/` tree; that is the backup. Properties that make this
easy:

- Objects are immutable and named by their BLAKE3 hash, so copying them is
  idempotent and a partially copied object is detected (every object is
  verified on read; `agit fsck` on a clone reports problems).
- Refs are written atomically. Copying `objects/` first and `refs/` second
  yields a backup in which every ref points at objects that exist, even while
  the server runs. The reverse order can produce dangling refs.
- `events.jsonl` is append-only; a copy taken mid-write is simply a shorter
  log.

Recommended approaches, in order of consistency:

1. **Filesystem snapshot** (ZFS, btrfs, LVM, EBS/cloud disk snapshot) of the
   volume: point-in-time, no downtime.
2. **Stop, copy, start** for small servers: `systemctl stop agit-server`,
   `tar` or `rsync` the data dir, start again. Takes seconds for most
   repositories.
3. **Live `rsync`** with the ordering above, run twice:
   `rsync -a --delete data/ backup/` then again; the second pass is small
   and closes the gap.

Ship copies off-host (restic, borg, rclone, cloud buckets); `webhooks.json`
holds webhook secrets, so encrypt backups at rest. Exclude nothing except
`locks/`. For Docker, back up the volume with
`docker run --rm -v agit-data:/data:ro -v "$PWD":/out alpine tar -C / -czf /out/agit-data.tgz data`.

**Restore:** stop the server, put the tree back at `AGIT_DATA_DIR`, fix
ownership (`chown -R agit:agit` or uid 10001 in Docker), start the server, then
`agit clone` a repository and run `agit fsck` in the clone to confirm. Verify
backups the same way on a schedule.

## Upgrades

1. Read [CHANGELOG.md](../CHANGELOG.md) for the target version. Before 1.0,
   a minor version may change on-disk formats; such changes are listed under
   "Changed" and say whether old data is read transparently.
2. Take a backup (above).
3. Replace the binary and restart:
   - binary/systemd: install the new `agit-server` (and `agit`), unpack the
     new web tarball if you serve the UI, `systemctl restart agit-server`;
   - Docker: `docker pull ghcr.io/shadow-kernel/agit:<version>`, recreate the
     container (`docker compose pull && docker compose up -d`); the volume
     carries the data.
4. Check `GET /api/v1/health` reports the new version and `agit pull` works
   from a client.

Clients and server should run the same minor version while the API is `v1`
and pre-1.0; the server rejects unknown object formats rather than guessing.
Downgrading is possible as long as the data format did not change in between;
otherwise restore the backup.

## Resource sizing

These are starting points, not measurements. Measured figures for the local
core are in [BENCHMARKS.md](BENCHMARKS.md); publish server numbers there once
you have them, and size from your own load.

What drives cost:

- **CPU:** zstd compression/decompression and BLAKE3 hashing of objects on
  push and fetch, three-way merges for PRs. Both are fast per byte; the server
  is rarely CPU-bound except during large imports.
- **Memory:** small baseline (tens of MB) plus buffers proportional to the
  object streams in flight and the number of open SSE connections. Every
  agent watching a repository holds one connection.
- **Disk:** objects are zstd-compressed; expect roughly the size of the
  equivalent git repository. Pushes and merges are fsync-bound
  (`AGIT_FSYNC=refs`), so IOPS and latency matter more than throughput: local
  SSD or a low-latency network disk, not a network share.
- **File descriptors:** one per connection plus per open object; the systemd
  unit sets `LimitNOFILE=65536`.

| Deployment | Guidance |
| --- | --- |
| Personal / demo | 1 vCPU, 512 MB, any SSD |
| Team, tens of agents | 2 vCPU, 1-2 GB, SSD with snapshots |
| Busy, hundreds of concurrent agents | 4+ vCPU, 4 GB, NVMe; put the proxy on the same host; watch SSE connection counts and p99 push latency |

Scale vertically for now: there is one process and one data directory per
server. Separate trust domains or very large installations get separate
servers behind different hostnames.

## Logging and monitoring

- Logs go to stdout/stderr (journald under systemd, `docker logs` in
  containers). `RUST_LOG=info` logs startup and warnings;
  `RUST_LOG=info,tower_http=debug` adds one line per request with status and
  latency.
- Liveness: `GET /api/v1/health` (with the token) returns
  `{"status":"ok","version":...}`.
- Activity: `GET /api/v1/repos/{repo}/events?since=<seq>` or the SSE stream
  gives a feed of commits, ref updates, PRs and leases for dashboards and
  alerting; `GET /api/v1/repos/{repo}/agents` lists recently active agents.
- Disk usage of `repos/*/objects` is the number to graph.

## Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| `403 forbidden` on every call, including `/health` | Token set on the server but missing or different on the client. Set `AGIT_TOKEN` (CLI), the client option (SDK) or the settings popover (web). Check the server was not started with an empty `AGIT_SERVER_TOKEN=`. |
| Container exits with a permission error on `/data` | Bind mount not writable by uid 10001: `chown -R 10001:10001 <dir>`. |
| Events arrive in bursts or the SSE stream drops behind a proxy | Response buffering or a read timeout on the proxy. Use the `events/stream` location from the nginx example or `flush_interval -1` in Caddy. |
| `413` or a failed push behind nginx | `client_max_body_size` too small; set it to `0` or a generous value. |
| Web UI loads but shows "Cannot reach ..." | The UI's API base URL points elsewhere. Open the settings popover and set `https://<host>/api/v1`, or rebuild with `VITE_AGIT_API`. |
| `ref_conflict` errors from clients | Expected under concurrent pushes; agents retry (`retryable: true`). Not a server problem. |
| Slow pushes on network storage | fsync latency. Move the data directory to local SSD; do not switch to `AGIT_FSYNC=none` on a server that holds the only copy. |
