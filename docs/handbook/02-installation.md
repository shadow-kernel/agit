# 2. Installation

agit ships as two static binaries: `agit` (the CLI, which also contains the MCP server and the git bridge) and `agit-server` (the hosting server). Pick one of the methods below. All of them end with `agit --version` printing `agit 0.1.0`.

## From source with cargo

Requirements: a Rust toolchain 1.89 or newer (the workspace sets `rust-version = "1.89"`; the README's "1.80 or newer" is out of date), and `git` on `PATH` if you want `agit git import|export`.

```sh
git clone https://github.com/shadow-kernel/agit
cd agit
cargo build --release                      # both binaries
export PATH="$PWD/target/release:$PATH"
agit --version
```

Only the CLI:

```sh
cargo build --release -p agit-cli          # target/release/agit
cargo install --path crates/agit-cli       # into ~/.cargo/bin
```

Only the server:

```sh
cargo build --release -p agit-server       # target/release/agit-server
```

`make release` is the same as `cargo build --release`; `make check` runs `cargo fmt --check`, `clippy -D warnings` and all tests, which is what CI does. The first build compiles about 300 crates and takes a few minutes; incremental builds are fast.

## Release binaries with `scripts/install.sh`

The installer downloads `agit-<version>-<target>.tar.gz` from the GitHub release, verifies it against the published `.sha256` file and installs both binaries.

```sh
curl -fsSL https://raw.githubusercontent.com/shadow-kernel/agit/main/scripts/install.sh | sh
```

Options (each also has an environment variable, useful with `curl | sh`):

| Option | Variable | Meaning |
| --- | --- | --- |
| `--version <tag or X.Y.Z>` | `AGIT_VERSION` | release to install (default: latest) |
| `--prefix <dir>` | `AGIT_PREFIX` | install root; binaries go to `<dir>/bin` (default `~/.local`, or `/usr/local` when root) |
| `--bin-dir <dir>` | `AGIT_BIN_DIR` | exact directory for the binaries, overrides `--prefix` |
| `--dry-run` | | print what would happen, download nothing |
| `--no-verify` | | skip checksum verification (not recommended) |

Supported targets: macOS arm64 and x86_64, Linux x86_64 and aarch64 (glibc). Windows users download `agit-<version>-x86_64-pc-windows-msvc.zip` from the releases page by hand; the CLI works on Windows, the Claude Code hooks need WSL or Git Bash.

Examples:

```sh
curl -fsSL .../install.sh | sh -s -- --version v0.1.0
curl -fsSL .../install.sh | sudo sh -s -- --prefix /usr/local
curl -fsSL .../install.sh | sh -s -- --dry-run
```

Make sure `~/.local/bin` is on your `PATH` when you keep the default prefix.

## Docker

Images are published to `ghcr.io/shadow-kernel/agit` for `linux/amd64` and `linux/arm64`, tagged `<version>`, `<major.minor>` and `latest`. The image contains `agit-server`, the `agit` CLI, `git` and the built web UI, runs as the unprivileged user `agit` (uid 10001), and has a health check against `/api/v1/health`.

```sh
docker run -d --name agit \
  -p 127.0.0.1:7878:7878 \
  -v agit-data:/data \
  -e AGIT_SERVER_TOKEN="$(openssl rand -hex 32)" \
  ghcr.io/shadow-kernel/agit:0.1.0
docker logs -f agit
curl -s http://127.0.0.1:7878/api/v1/health
```

Image defaults: `AGIT_DATA_DIR=/data`, `AGIT_LISTEN=0.0.0.0:7878`, `AGIT_WEB_DIR=/opt/agit/web`, `RUST_LOG=info,tower_http=info`. Container arguments go to `agit-server`, so `docker run --rm ghcr.io/shadow-kernel/agit --help` works. To use the CLI from the image:

```sh
docker exec agit agit --version
docker run --rm -v "$PWD":/work -w /work --entrypoint agit ghcr.io/shadow-kernel/agit:0.1.0 status --json
```

Build locally from a checkout (compiles the workspace in release mode inside the image; cargo and pnpm caches are BuildKit cache mounts):

```sh
docker build -t agit:dev .
docker build --build-arg AGIT_WEB_API=https://agit.example.com/api/v1 -t agit:dev .   # API URL baked into the UI
```

`docker-compose.yml` in the repository root runs the image with a named volume and passes `AGIT_SERVER_TOKEN` through from the host environment; see [13. Server administration](13-server-administration.md#docker-and-compose).

## Homebrew (template)

`homebrew/agit.rb` is a formula template. The release workflow renders it with the real version and SHA-256 values and attaches it to each GitHub release as `agit.rb`; publishing it to a tap is a manual step. Once a tap exists:

```sh
brew install shadow-kernel/tap/agit                       # from the tap, when published
brew install --formula https://github.com/shadow-kernel/agit/releases/download/v0.1.0/agit.rb
brew services start agit                                  # local server on 127.0.0.1:7878, data in $(brew --prefix)/var/agit
```

The formula installs both binaries, recommends `git`, and defines a `brew services` unit that runs `agit-server` with `AGIT_DATA_DIR=$(brew --prefix)/var/agit`, `AGIT_LISTEN=127.0.0.1:7878` and logs in `var/log/agit-server.log`. Set `AGIT_SERVER_TOKEN` in the service environment if the laptop is shared.

## Shell completion

agit does not generate shell completions in 0.1.0. There is no `agit completions` subcommand and no `complete` flag; `agit --help` and `agit <command> --help` are the discovery tools. (The CLI is built with clap, so completion generation is a small addition; it is not on the roadmap yet.)

## First checks

```sh
$ agit --version
agit 0.1.0
$ agit-server --help
agit hosting server

Usage: agit-server [OPTIONS]

Options:
      --data-dir <DATA_DIR>  Directory where repositories are stored [env: AGIT_DATA_DIR=] [default: ./agit-data]
      --listen <LISTEN>      Listen address [env: AGIT_LISTEN=] [default: 127.0.0.1:7878]
      --token <TOKEN>        Bearer token required on every request (unauthenticated when unset) [env: AGIT_SERVER_TOKEN=]
      --web-dir <WEB_DIR>    Serve the built web UI (web/dist) from `/` [env: AGIT_WEB_DIR=]
```

Set an author identity once per repository (`agit config user.name`, `agit config user.email`) or globally through the environment (`AGIT_AUTHOR_NAME`, `AGIT_AUTHOR_EMAIL`); without either, commits use the OS user name. Agents additionally set `AGIT_AGENT` (and optionally `AGIT_MODEL`, `AGIT_RUN_ID`), which is covered in [4. Quick start for agents](04-quickstart-agents.md).

## Optional components

| Component | Where | Needs |
| --- | --- | --- |
| Web UI | `web/` | Node 22 and pnpm (`cd web && pnpm install && pnpm build`), or the `agit-web-<version>.tar.gz` release asset; served with `agit-server --web-dir web/dist` |
| Python SDK | `sdk/python` | Python 3.10+, `pip install -e sdk/python` (no runtime dependencies) |
| TypeScript SDK | `sdk/typescript` | Node 18+/Deno/Bun, `pnpm add @agit/sdk` or `pnpm install && pnpm build` in the directory |
| MCP server | built into `agit` | an MCP client; `agit mcp` runs over stdio ([7](07-mcp-reference.md)) |
| Claude Code hooks | `integrations/claude-code/` | bash 3.2+, `python3` (or `jq`); `install.sh` ([7](07-mcp-reference.md#claude-code-integration)) |

Upgrading is "replace the binaries". Before 1.0 a minor version may change on-disk formats; the [CHANGELOG](../../CHANGELOG.md) says so under "Changed" when it happens, and [13. Server administration](13-server-administration.md#upgrades) has the server procedure.
