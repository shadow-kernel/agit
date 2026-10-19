# syntax=docker/dockerfile:1.7
#
# agit server image: agit-server + agit CLI + the built web UI.
#
#   docker build -t agit:dev .
#   docker run --rm -p 7878:7878 -v agit-data:/data agit:dev
#
# Stages:
#   web       builds web/dist with node 22 + pnpm (corepack)
#   source    builds the Rust release binaries from this checkout
#   prebuilt  takes binaries from dist/linux-<arch>/ instead (used by the
#             release workflow so multi-arch images do not compile under QEMU)
#   final     debian:bookworm-slim, non-root, git installed for `agit git`
#
# Select the binary source with --build-arg BINARIES=source|prebuilt.

ARG BINARIES=source
ARG RUST_IMAGE=rust:1-bookworm
ARG NODE_IMAGE=node:22-bookworm-slim
ARG RUNTIME_IMAGE=debian:bookworm-slim

# ---------------------------------------------------------------------------
# Web UI
# ---------------------------------------------------------------------------
FROM --platform=$BUILDPLATFORM ${NODE_IMAGE} AS web
WORKDIR /src/web
# Pin the pnpm major that CI uses and that web/pnpm-lock.yaml was written
# with (package.json has no packageManager field, so corepack would otherwise
# pick the newest pnpm, whose default supply-chain policy rejects freshly
# published dependencies).
ARG PNPM_VERSION=10.33.1
ENV COREPACK_ENABLE_DOWNLOAD_PROMPT=0
RUN corepack enable && corepack prepare "pnpm@${PNPM_VERSION}" --activate
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,id=pnpm-store,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY web/ ./
# The UI is served by agit-server from the same origin, so talk to the API at
# a relative path. Override for a split deployment:
#   --build-arg AGIT_WEB_API=https://agit.example.com/api/v1
ARG AGIT_WEB_API=/api/v1
ENV VITE_AGIT_API=${AGIT_WEB_API}
RUN pnpm build

# ---------------------------------------------------------------------------
# Rust binaries built from source
# ---------------------------------------------------------------------------
FROM ${RUST_IMAGE} AS source
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
# The target dir is a cache mount, so copy the binaries out in the same step.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=cargo-target,target=/src/target \
    cargo build --release --locked -p agit-cli -p agit-server \
    && mkdir -p /out \
    && cp target/release/agit target/release/agit-server /out/

# ---------------------------------------------------------------------------
# Rust binaries supplied by the caller (release workflow)
# Expects dist/linux-amd64/{agit,agit-server} and dist/linux-arm64/{...}.
# ---------------------------------------------------------------------------
FROM scratch AS prebuilt
ARG TARGETARCH
COPY --chmod=0755 dist/linux-${TARGETARCH}/agit dist/linux-${TARGETARCH}/agit-server /out/

FROM ${BINARIES} AS binaries

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
FROM ${RUNTIME_IMAGE} AS final

LABEL org.opencontainers.image.title="agit" \
      org.opencontainers.image.description="Version control and collaboration for AI agents: agit-server, agit CLI and web UI" \
      org.opencontainers.image.source="https://github.com/shadow-kernel/agit" \
      org.opencontainers.image.licenses="MIT OR Apache-2.0"

# git: `agit git import|export` shells out to it. curl: the HEALTHCHECK below.
# tini: reaps children and forwards SIGTERM so `docker stop` is immediate.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 agit \
    && useradd --system --uid 10001 --gid agit --home-dir /data --shell /usr/sbin/nologin agit \
    && mkdir -p /data /opt/agit/web \
    && chown -R agit:agit /data

COPY --from=binaries /out/agit /out/agit-server /usr/local/bin/
COPY --from=web /src/web/dist/ /opt/agit/web/

ENV AGIT_DATA_DIR=/data \
    AGIT_LISTEN=0.0.0.0:7878 \
    AGIT_WEB_DIR=/opt/agit/web \
    RUST_LOG=info,tower_http=info

VOLUME ["/data"]
EXPOSE 7878
USER agit:agit
WORKDIR /data

# /api/v1/health is public, so no credentials are needed here.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS "http://127.0.0.1:${AGIT_LISTEN##*:}/api/v1/health" >/dev/null || exit 1

ENTRYPOINT ["/usr/bin/tini", "--", "agit-server"]
